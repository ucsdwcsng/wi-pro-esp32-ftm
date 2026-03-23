import numpy as np
from pathlib import Path

def export_idft_svd(start_dist, stop_dist, num_points, tol=1e-3):
    base_dir = Path(__file__).parent / '../firmware/data'
    base_dir = base_dir.resolve()
    base_dir.mkdir(parents=True, exist_ok=True)

    d_range = np.linspace(start_dist, stop_dist, num_points)
    freq = np.fft.fftfreq(128, d=1/40e6)
    M = np.exp(2j * np.pi * d_range[:, None] / 3e8 * freq[None, :])

    # Compute sinc rise compensation
    RISE_THRESH = 0.25
    sinc = np.abs(M @ np.ones(128))
    sinc /= np.max(sinc)
    sinc_rise_comp = -d_range[np.argmax(sinc > RISE_THRESH)]
    print(f"sinc_rise_comp: {sinc_rise_comp:.4f} m")

    U, s, Vh = np.linalg.svd(M, full_matrices=False)
    energy = np.cumsum(s**2) / np.sum(s**2)
    r = int(np.searchsorted(energy, 1.0 - tol) + 1)
    print(f"Rank {r} captures {energy[r-1]*100:.2f}% of energy")

    L = U[:, :r] * s[None, :r]
    R = Vh[:r, :]

    def save_complex(arr, path):
        out = np.zeros(arr.size * 2, dtype=np.float32)
        out[0::2] = arr.flatten().real.astype(np.float32)
        out[1::2] = arr.flatten().imag.astype(np.float32)
        out.tofile(path)

    save_complex(L, base_dir / 'idft_L.bin')
    save_complex(R, base_dir / 'idft_R.bin')
    np.array([num_points, 128, r], dtype=np.uint32).tofile(base_dir / 'idft_meta.bin')
    np.array(d_range, dtype=np.float32).tofile(base_dir / 'idft_drange.bin')
    np.array([sinc_rise_comp], dtype=np.float32).tofile(base_dir / 'idft_sinc_rise_comp.bin')
    print(f"L: {num_points*r*8} bytes, R: {r*128*8} bytes")

if __name__ == "__main__":
    export_idft_svd(-10, 60, 512)
