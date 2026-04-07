use base64::{engine::general_purpose, Engine as _};
use crate::ftm::FtmReport;
use crate::csi::{FTM_CSI_STATE, CsiData};
use esp_idf_svc::sys as esp_idf_sys;
use log::info;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use nalgebra::{SVector,DMatrix};
use num_complex::Complex;

type HVec40 = SVector<Complex<f32>, 128>;

static FFT_INIT: std::sync::Once = std::sync::Once::new();

extern "C" {
    static mut dsps_fft_w_table_fc32: *mut f32;
}

pub struct WiproState {
    pub num_l1_iters: u32
}

pub static WIPRO_STATE: Mutex<CriticalSectionRawMutex, WiproState> =
    Mutex::new(WiproState {
	num_l1_iters: 0
    });


pub struct CompressedIdft {
    pub left: DMatrix<Complex<f32>>,   // num_points × r
    pub right: DMatrix<Complex<f32>>,  // r × 128
    pub d_range: Vec<f32>,
    pub sinc_rise_comp: f32,
}

// Baked into flash as .rodata — zero-cost at boot
static L_BYTES: &[u8] = include_bytes!("../data/idft_L.bin");
static R_BYTES: &[u8] = include_bytes!("../data/idft_R.bin");
static DRANGE_BYTES: &[u8] = include_bytes!("../data/idft_drange.bin");
static SINC_RISE_COMP_BYTES: &[u8] = include_bytes!("../data/idft_sinc_rise_comp.bin");

static IDFT: std::sync::OnceLock<CompressedIdft> = std::sync::OnceLock::new();

fn bytes_to_complex_vec(bytes: &[u8]) -> Vec<Complex<f32>> {
    let count = bytes.len() / 8;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let re = f32::from_le_bytes(bytes[i*8..i*8+4].try_into().unwrap());
        let im = f32::from_le_bytes(bytes[i*8+4..i*8+8].try_into().unwrap());
        out.push(Complex::new(re, im));
    }
    out
}

fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
    let count = bytes.len() / 4;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        out.push(f32::from_le_bytes(bytes[i*4..i*4+4].try_into().unwrap()));
    }
    out
}

fn bytes_to_u32_slice(bytes: &[u8]) -> [u32; 3] {
    [
        u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
        u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
    ]
}

pub fn get_or_init_idft() -> &'static CompressedIdft {
    IDFT.get_or_init(|| {
        let meta = bytes_to_u32_slice(include_bytes!("../data/idft_meta.bin"));
        let num_points = meta[0] as usize;
        let r = meta[2] as usize;
        info!("Loading compressed IDFT: {}x128 via rank {}", num_points, r);
        load_compressed_idft(num_points, r)
    })
}


pub fn load_compressed_idft(num_points: usize, r: usize) -> CompressedIdft {
    let l_data = bytes_to_complex_vec(L_BYTES);
    let r_data = bytes_to_complex_vec(R_BYTES);
    let d_data = bytes_to_f32_vec(DRANGE_BYTES);
    let sinc_rise_comp = f32::from_le_bytes(
        SINC_RISE_COMP_BYTES[0..4].try_into().unwrap(),
    );

    let left = DMatrix::from_row_slice(num_points, r, &l_data);
    let right = DMatrix::from_row_slice(r, 128, &r_data);

    CompressedIdft {
        left,
        right,
        d_range: d_data,
	sinc_rise_comp,
    }
}


/// Apply: result = L @ (R @ h), where h is 128×1
pub fn apply_idft(idft: &CompressedIdft, h: &HVec40) -> Vec<Complex<f32>> {
    let r = idft.right.nrows();
    let num_points = idft.left.nrows();

    // Step 1: R @ h → mid (r × 1)
    let mut mid = vec![Complex::new(0.0f32, 0.0f32); r];
    for i in 0..r {
        let mut sum = Complex::new(0.0f32, 0.0f32);
        for j in 0..128 {
            sum += idft.right[(i, j)] * h[j];
        }
        mid[i] = sum;
    }

    // Step 2: L @ mid → out (num_points × 1)
    let mut out = vec![Complex::new(0.0f32, 0.0f32); num_points];
    for i in 0..num_points {
        let mut sum = Complex::new(0.0f32, 0.0f32);
        for k in 0..r {
            sum += idft.left[(i, k)] * mid[k];
        }
        out[i] = sum;
    }
    out
}

pub fn fft_init() {
    FFT_INIT.call_once(|| {
        unsafe {
            esp_idf_sys::dsps_fft2r_init_fc32(core::ptr::null_mut(), 128);
        }
    });
}

fn load_h_40(entry: &CsiData) -> Option<Box<HVec40>> {
    let mut h = Box::new(SVector::<Complex<f32>, 128>::zeros());
    if entry.len != 384 {return None;}
    for ii in 0..128 {
        let imag = (entry.buf[128 + 2 * ii] as i8) as f32;
        let real = (entry.buf[128 + 2 * ii + 1] as i8) as f32;
        h[ii] = Complex::new(real, imag);
    }
    let mut sum = 0.0;
    for ii in 0..128 {
	sum += h[ii].norm();
    }
    let sum_c = Complex::new(sum, 0.0f32);
    for ii in 0..128 {
	h[ii] /= sum_c;
    }
    let j = Complex::new(0.0f32, 1.0f32);
    for ii in 64..128 {
	h[ii] *= j;
    }
    Some(h)
}


fn fft_ortho(h: &HVec40) -> Box<HVec40> {
    const N: usize = 128;
    let scale = 1.0f32 / (N as f32).sqrt();

    let mut buf = vec![0.0f32; N * 2];
    for i in 0..N {
        buf[2 * i]     = h[i].re;
        buf[2 * i + 1] = h[i].im;
    }

    unsafe {
        esp_idf_sys::dsps_fft2r_fc32_ae32_(buf.as_mut_ptr(), N as i32, dsps_fft_w_table_fc32);
        esp_idf_sys::dsps_bit_rev2r_fc32(buf.as_mut_ptr(), N as i32);
    }

    let mut out = Box::new(HVec40::zeros());
    for i in 0..N {
        out[i] = Complex::new(
            buf[2 * i]     * scale,
            buf[2 * i + 1] * scale,
        );
    }
    out
}


fn ifft_ortho(h: &HVec40) -> Box<HVec40> {
    const N: usize = 128;
    let scale = 1.0f32 / (N as f32).sqrt();

    // Copy into a raw interleaved buffer and conjugate (IFFT trick)
    let mut buf = vec![0.0f32; N * 2];
    for i in 0..N {
        buf[2 * i]     =  h[i].re;
        buf[2 * i + 1] = -h[i].im;  // conjugate
    }

    // Call esp-dsp FFT
    unsafe {
        esp_idf_sys::dsps_fft2r_fc32_ae32_(buf.as_mut_ptr(), N as i32, dsps_fft_w_table_fc32);
        esp_idf_sys::dsps_bit_rev2r_fc32(buf.as_mut_ptr(), N as i32);
    }

    // Conjugate output and apply ortho scale
    let mut out = Box::new(HVec40::zeros());
    for i in 0..N {
        out[i] = Complex::new(
             buf[2 * i]     * scale,
            -buf[2 * i + 1] * scale,  // conjugate + scale
        );
    }
    out
}

const Z_IDX_40: [usize; 114] = [2,   3,   4,   5,   6,   7,   8,   9,  10,  11,  12,  13,  14,
				15,  16,  17,  18,  19,  20,  21,  22,  23,  24,  25,  26,  27,
				28,  29,  30,  31,  32,  33,  34,  35,  36,  37,  38,  39,  40,
				41,  42,  43,  44,  45,  46,  47,  48,  49,  50,  51,  52,  53,
				54,  55,  56,  57,  58,  70,  71,  72,  73,  74,  75,  76,  77,
				78,  79,  80,  81,  82,  83,  84,  85,  86,  87,  88,  89,  90,
				91,  92,  93,  94,  95,  96,  97,  98,  99, 100, 101, 102, 103,
				104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
				117, 118, 119, 120, 121, 122, 123, 124, 125, 126];

async fn l1_interp(h: &mut HVec40) {
    let iters = WIPRO_STATE.lock().await.num_l1_iters;
    let mut eps = Complex::new(1e-1f32, 0.0f32);
    let mut iter = 0;
    let mut fail = 0;
    let mut loss = std::f32::INFINITY;
    while iter < iters {
	info!("iter {}",iter);
	let mut sgn_ax = ifft_ortho(h);
	for ji in 0..128 {
	    let _nrm = sgn_ax[ji].norm() + 1e-9;
	    sgn_ax[ji] /= _nrm;
	}
	let mut grad_ = fft_ortho(&sgn_ax);
	for ji in 0..Z_IDX_40.len() {
	    grad_[Z_IDX_40[ji]] = Complex::new(0.0f32, 0.0f32);
	}
	
	let h_test = h.clone() - *grad_ * eps;
	let loss_ = ifft_ortho(&h_test).map(|c| c.norm()).sum();
	if loss_ < loss {
	    eps *= 1.1;
	    iter += 1;
	    h.copy_from(&h_test);
	    loss = loss_;
	    fail = 0;
	} else {
	    eps *= 0.5;
	    fail += 1;
	    if fail > 10 {
		break;
	    }
	}
    }
}

fn diff_48bit(a: i64, b: i64) -> i64 {
    let unsigned_diff = a.wrapping_sub(b) & 0xffffffffffff;
    if unsigned_diff >= 0x800000000000 {
        unsigned_diff as i64 - 0x1000000000000i64
    } else {
        unsigned_diff as i64
    }
}

pub fn shift_to_zero(h: &mut HVec40, rtt_ps: i64) {
    const N: usize = 128;
    const SAMPLE_SPACING: f64 = 1.0 / 40e6; // d = 1/40 MHz = 25 ns

    let delay_s = (rtt_ps as f64) / 2e12; // one-way delay in seconds

    for k in 0..N {
        // np.fft.fftfreq(128, d=1/40e6)
        let freq = if k < N / 2 {
            k as f64 / (N as f64 * SAMPLE_SPACING)
        } else {
            (k as f64 - N as f64) / (N as f64 * SAMPLE_SPACING)
        };

        let phase = -2.0 * std::f64::consts::PI * freq * delay_s;
        let (sin_p, cos_p) = phase.sin_cos();
        let rot = Complex::new(cos_p as f32, sin_p as f32);
        h[k] *= rot;
    }
}

pub fn estimate_range(cir: &[Complex<f32>], idft: &CompressedIdft) -> f32 {
    const RISE_THRESH: f32 = 0.25;

    let abs_cir: Vec<f32> = cir.iter().map(|c| c.norm()).collect();
    let max_val = abs_cir.iter().cloned().fold(0.0f32, f32::max);

    let first_above = abs_cir
        .iter()
        .position(|&v| v / max_val > RISE_THRESH)
        .unwrap_or(0);

    idft.d_range[first_above] + idft.sinc_rise_comp
}

pub async fn process_report(report: &FtmReport) {
    // Snapshot the CSI buffer while holding the lock as briefly as possible.
    // Never hold a mutex guard across an `.await`.
    let csi_entries: Vec<CsiData> = {
        let guard = FTM_CSI_STATE.lock().await;
        let state = guard.borrow();
        state.buffer.clone()   // or core::mem::take if you want to drain it here
    };

    let mut mean_range: f32 = 0.0;

    for ii in 0..report.meta.num_entries as usize {
	let ftm_entry = report.entries[ii];

	// find matching CSI entry in the buffer
	let csi_entry = {
	    let mut idx_match: i32 = -1;
	    for ji in 0..csi_entries.len() {
		let csi_stamp = crate::csi::calculate_precise_timestamp_ns(&csi_entries[ji], true) * 1000;
		let ftm_stamp = ftm_entry.t2;
		let delta = ftm_stamp.wrapping_sub(csi_stamp) as i64;
		if delta > -2000 && delta < 2000 {
		    idx_match = ji as i32;
		    break;
		}
	    }
	    if idx_match == -1 {
		continue
	    }
	    &csi_entries[idx_match as usize]
	};

	let Some(mut h) = load_h_40(csi_entry) else {continue};
	l1_interp(&mut h);
	let rtt_ps = diff_48bit(ftm_entry.t4 as i64, ftm_entry.t1 as i64) - 
	    diff_48bit(ftm_entry.t3 as i64, ftm_entry.t2 as i64);
	shift_to_zero(&mut h, rtt_ps);
	let idft = get_or_init_idft();
	let cir = apply_idft(idft, &h);
	let range = estimate_range(&cir,idft);
	mean_range += range;
	print!(
	    "\x02RANGE\x01{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}\x01{}\x01{}\x03\r\n",
	    report.meta.peer_mac[0], report.meta.peer_mac[1], report.meta.peer_mac[2],
	    report.meta.peer_mac[3], report.meta.peer_mac[4], report.meta.peer_mac[5],
	    ftm_entry.t2,
	    range
	);
    }
    if report.meta.num_entries > 0 {
	info!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}: {} meters",
	      report.meta.peer_mac[0], report.meta.peer_mac[1], report.meta.peer_mac[2],
	      report.meta.peer_mac[3], report.meta.peer_mac[4], report.meta.peer_mac[5],
	      mean_range / report.meta.num_entries as f32);
    }
}

pub fn _dump_debug_fc32(data: &[Complex<f32>], len: usize) {
    let byte_len = len * core::mem::size_of::<Complex<f32>>();
    let raw_bytes: &[u8] = unsafe {
        core::slice::from_raw_parts(data.as_ptr() as *const u8, byte_len)
    };

    let base64_len = (byte_len * 4 / 3) + 4;
    let mut base64_buf = vec![0u8; base64_len];
    
    match general_purpose::STANDARD.encode_slice(raw_bytes, &mut base64_buf) {
        Ok(encoded_len) => {
            if let Ok(encoded_str) = core::str::from_utf8(&base64_buf[..encoded_len]) {
                let msg = format!(
                    "\x02\
		     DBG\x01\
                     fc32\x01\
                     {}\x01\
                     {}\x03\r\n",
                    len,
                    encoded_str
                );
                print!("{}", msg);
            } else {
                info!("dump_debug_fc32: Failed to convert base64 to UTF-8");
            }
        }
        Err(e) => {
            info!("dump_debug_fc32: Base64 encoding failed: {:?}", e);
        }
    }
}

pub async fn set_l1_iters(iters: u32) {
    let mut state = WIPRO_STATE.lock().await;
    state.num_l1_iters = iters;
    info!("L1 iters: {}", iters);
}
