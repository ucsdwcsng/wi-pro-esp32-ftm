import numpy as np
import csv
from base64 import b64decode
from os.path import join
import tqdm
from dataclasses import dataclass, field
import matplotlib.pyplot as plt
import matplotlib.animation as ani
import time
import msgpack

def diff_48bit(a, b):
    unsigned_diff = (a - b) & 0xffffffffffff
    # If MSB is set, it's negative in two's complement
    if unsigned_diff >= 0x800000000000:
        return unsigned_diff - 0x1000000000000
    else:
        return unsigned_diff

@dataclass
class FTMReport:
    own_mac: str
    tgt_mac: str
    dlog_token: int
    rssi: int
    t1: int
    t2: int
    t3: int
    t4: int
    channel: int
    channel2: int
    csi_b64: str
    mac_timestamp: int

    def get_delay_offset(self):
        delay = (diff_48bit(self.t4, self.t1) - diff_48bit(self.t3, self.t2)) // 2
        offset = diff_48bit(diff_48bit(self.t2,self.t1),diff_48bit(self.t4,self.t3))
        return delay, offset
    

@dataclass
class FTMEvent:
    t_ms: int
    own_mac: str
    tgt_mac: str
    seq: int
    reports: list[FTMReport] = field(default_factory=list)

    def __init__(self, data: bytes):
        try:
            data_raw = msgpack.unpackb(data, raw=False)
        except Exception as e:
            raise ValueError(f"Error deserializing data: {e}") from e

        self.t_ms = int(data_raw[0])
        self.own_mac = data_raw[1]
        self.tgt_mac = data_raw[2]  # was tgt_pac
        self.seq = data_raw[3]
        self.reports = [
            FTMReport(
                own_mac=rep[0],
                tgt_mac=rep[1],
                dlog_token=rep[2],
                rssi=rep[3],
                t1=rep[4],
                t2=rep[5],
                t3=rep[6],
                t4=rep[7],
                channel=rep[8],
                channel2=rep[9],
                csi_b64=rep[10],
                mac_timestamp=rep[11],
            )
            for rep in data_raw[4]
        ]

@dataclass
class CSIEvent:
    t_ms: int
    seq: int
    own_mac: str
    tgt_mac: str
    timestamp: int
    channel: int
    channel2: int
    rssi: int
    payload_b64: str
    mac_timestamp: int
    sig_mode: int

def print_ftm_counts(path):
    mac_counts={}
    with open(join(path, 'ftm.csv'), 'r') as f:
        reader = csv.reader(f)
        for row in reader:
            mac = row[1]
            if mac not in mac_counts.keys():
                mac_counts[mac] = 0
            else:
                mac_counts[mac] = mac_counts[mac] + 1

    print("FTMs received per MAC:")
    for m in mac_counts:
        print(f"{m}: {mac_counts[m]}")

def shift_interp(h, shift):
    h[64:] *= np.exp(1.0j*shift)
    h_up = h[2]
    h_down = h[-2]
    h[0] = h_up/2 + h_down/2
    h[1] = h_up*3/4 + h_down*1/4
    h[-1] = h_up*1/4 + h_down*3/4
    h /= h[0,None]
    return h

def l1_err(h):
    return np.sum(np.abs(np.fft.ifft(h)))


IDFT_MAT = np.exp(2.0j*np.pi*np.arange(128)[:,None]*np.arange(128)[None,:]/128)/np.sqrt(128)
EXCLUDED_SUBC = np.in1d(np.arange(128),[0,1,59,60,61,62,63,64,65,66,67,68,69,127])
ZIDX = np.logical_not(EXCLUDED_SUBC)
def optimal_interp_edges(h, iters=20):
    eps = 0.005
    h_ = np.copy(h)

    ii = 0
    fail = 0
    loss = np.inf
    # hs = []
    # Hs = []
    # losses = []
    while(ii < iters):
        sgn_ax_ = np.fft.ifft(h_,norm='ortho')
        sgn_ax_ = sgn_ax_ / (np.abs(sgn_ax_) + 1e-9)
        grad_ = np.fft.fft(sgn_ax_,norm='ortho')
        grad_[ZIDX] = 0.0
        loss_new = np.sum(np.abs(np.fft.ifft(h_ - eps*grad_)))
        if(loss_new <= loss):
            #print(f"{ii} gain at eps={eps}")
            eps *= 1.1
            ii += 1
            h_ -= eps*grad_
            loss = loss_new
            fail = 0
            # losses.append(loss)
            # hs.append(np.fft.ifft(np.copy(h_)))
            # Hs.append(np.copy(h_))
        else:
            eps *= 0.5
            fail = fail + 1
            if fail > 10:
                break

    return h_

def normalize_csi_40(h, stitching='l1_edges',iters=20):
    h_interp = np.zeros_like(h)
    elif stitching == 'l1_edges':
        for hi in tqdm.trange(h.shape[0]):
            h_ = np.copy(h[hi,:])
            h_[64:] *= np.exp(1.0j*np.pi/2)
            h_ /= np.sum(np.abs(h_))
            h_interp[hi,:] = optimal_interp_edges(h_,iters=iters)

    elif stitching == 'constant':
        for hi in range(h.shape[0]):
            h_ = np.copy(h[hi,:])
            h_[64:] *= np.exp(1.0j*np.pi/2)
            h_ /= np.sum(np.abs(h_))
            h_interp[hi,:] = h_
    elif stitching == 'phase_slope':
        for hi in range(h.shape[0]):
            h_ = np.copy(h[hi,:])
            h_[64:] *= np.exp(1.0j*np.pi/2)
            h_ /= np.sum(np.abs(h_))
            ang = np.fft.fftshift(np.unwrap(np.angle(h_)))
            idx = np.arange(-64,64)
            line = np.linalg.lstsq(np.vstack([idx,np.ones(128)]).T, ang)[0]
            exp = np.fft.fftshift(np.exp(1.0j*(idx * line[0] + line[1])))
            mean_abs = np.mean(np.abs(h_[ZIDX]))
            h_[EXCLUDED_SUBC] = exp[EXCLUDED_SUBC]*mean_abs
            h_interp[hi,:] = h_
        pass
    else:
        assert False, "invalid mode"

    return h_interp

def normalize_csi_20(h):
    h = h.conj()
    h_up = np.mean(h[:,33:35], axis=1)
    h_down = np.mean(h[:,30:32], axis=1)
    h_ang = np.exp(1.0j*np.angle(h_up + h_down))
    h_abs = (np.abs(h_up) + np.abs(h_down))/2
    #if h_abs < 1.0:
    #    h_abs = 1.0
    h_dc = h_abs * h_ang
    h /= h_dc[:,None]
    h[:,32] = 1.0
    h /= np.linalg.norm(h)
    return h
        
def generate_ftm_csi_pairs(path, target_mac):
    h_ms = []
    h_stamp = []
    h_seq = []
    h_H = []
    with open(join(path, 'csi.csv'),'r') as f:
        reader = csv.reader(f)
        for row in reader:
            if row[2] == target_mac:
                h_ms.append(int(row[0]))
                h_stamp.append(int(row[1]))
                h_seq.append(int(row[3]))
                h = np.frombuffer(b64decode(row[4]), dtype=np.int8)
                #h = h[::2] + 1.0j * h[1::2]
                h = h[1::2] + 1.0j * h[0::2]
                h_H.append(h)
    t_ms = []
    t_dlog = []
    t_t0 = []
    t_t1 = []
    t_t2 = []
    t_t3 = []
    t_rssi = []
    
    with open(join(path, 'ftm.csv'), 'r') as f:
        reader = csv.reader(f)
        for row in reader:
            if row[3] == target_mac:
                t_dlog.append(int(row[4]))
                t_ms.append(int(row[1]))
                t_t0.append(int(row[6]))
                t_t1.append(int(row[7]))
                t_t2.append(int(row[8]))
                t_t3.append(int(row[9]))
                t_rssi.append(int(row[5]))
    h_stamp = np.asarray(h_stamp)
    t_stamp = np.asarray(t_t1)//1000

    # associate each CSI with the corresponding timestamp
    f_ms = []
    f_rssi = []
    f_h = []
    f_t = []
    for ii in range(len(t_stamp)):
        delta = np.abs(t_stamp[ii] - h_stamp)
        dmin = np.min(delta)
        if dmin > 2:
            print(f"Couldn't find good match for FTM at {t_stamp[ii]}, skipping...")
            continue
        h_idx = np.argmin(delta)
        #f_ms.append(h_ms[h_idx])
        f_ms.append(t_ms[ii])
        f_h.append(h_H[h_idx])
        f_t.append([t_t0[ii],t_t1[ii],t_t2[ii],t_t3[ii]])
        f_rssi.append(t_rssi[ii])
    return f_ms, f_t, f_h, f_rssi


def load_pps(filepath):
    data = {
        'own_mac': [],
        't_ms': [],
        'timestamp_esp': [],
        'timestamp_mac': [],
        'internal_offset': [],
        'compensated_mac_time': [],
        'frac': []
    }
    
    with open(join(filepath,"pps.csv"), 'r') as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
                
            parts = line.split(',')
            if len(parts) != 7:
                continue
                
            data['own_mac'].append(parts[0])
            data['t_ms'].append(int(parts[1]))
            data['timestamp_esp'].append(int(parts[2]))
            data['timestamp_mac'].append(int(parts[3]))
            data['internal_offset'].append(int(parts[4]))
            data['compensated_mac_time'].append(int(parts[5]))
            data['frac'].append(int(parts[6]))
    
    # Convert to numpy arrays
    data['own_mac'] = np.array(data['own_mac'], dtype=str)
    data['t_ms'] = np.array(data['t_ms'], dtype=np.uint64)
    data['timestamp_esp'] = np.array(data['timestamp_esp'], dtype=np.int64)
    data['timestamp_mac'] = np.array(data['timestamp_mac'], dtype=np.int64)
    data['internal_offset'] = np.array(data['internal_offset'], dtype=np.int64)
    data['compensated_mac_time'] = np.array(data['compensated_mac_time'], dtype=np.int64)
    data['frac'] = np.array(data['frac'], dtype=np.int64)
    
    return data

import numpy as np

def load_ftm(filepath):
    """
    Parse FTM CSV file into a dictionary of numpy arrays.
    
    Args:
        filepath: Path to the CSV file
        
    Returns:
        dict with keys: 'own_mac', 't_ms', 'seq', 'tgt_mac', 'dlog_token',
                       'rssi', 't1', 't2', 't3', 't4'
    """
    data = {
        'own_mac': [],
        't_ms': [],
        'seq': [],
        'tgt_mac': [],
        'dlog_token': [],
        'rssi': [],
        't1': [],
        't2': [],
        't3': [],
        't4': []
    }
    
    with open(join(filepath,'ftm.csv'), 'r') as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
                
            parts = line.split(',')
            if len(parts) != 10:
                continue
                
            data['own_mac'].append(parts[0])
            data['t_ms'].append(int(parts[1]))
            data['seq'].append(int(parts[2]))
            data['tgt_mac'].append(parts[3])
            data['dlog_token'].append(int(parts[4]))
            data['rssi'].append(int(parts[5]))
            data['t1'].append(int(parts[6]))
            data['t2'].append(int(parts[7]))
            data['t3'].append(int(parts[8]))
            data['t4'].append(int(parts[9]))
    
    # Convert to numpy arrays
    data['own_mac'] = np.array(data['own_mac'], dtype=str)
    data['t_ms'] = np.array(data['t_ms'], dtype=np.uint64)
    data['seq'] = np.array(data['seq'], dtype=np.uint32)
    data['tgt_mac'] = np.array(data['tgt_mac'], dtype=str)
    data['dlog_token'] = np.array(data['dlog_token'], dtype=np.int32)
    data['rssi'] = np.array(data['rssi'], dtype=np.int32)
    data['t1'] = np.array(data['t1'], dtype=np.int64)
    data['t2'] = np.array(data['t2'], dtype=np.int64)
    data['t3'] = np.array(data['t3'], dtype=np.int64)
    data['t4'] = np.array(data['t4'], dtype=np.int64)
    
    return data


def load_ftm_events(filepath) -> list[FTMEvent]:
    """
    Load FTM CSV into a list of FTMEvent dataclasses.
    Fields not saved in the CSV (channel, channel2, csi_b64, mac_timestamp) are set to placeholder values.
    """
    events = []
    with open(filepath, 'r') as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            parts = line.split(',')
            if len(parts) != 10:
                continue

            report = FTMReport(
                own_mac=parts[0],
                tgt_mac=parts[3],
                dlog_token=int(parts[4]),
                rssi=int(parts[5]),
                t1=int(parts[6]),
                t2=int(parts[7]),
                t3=int(parts[8]),
                t4=int(parts[9]),
                channel=0,
                channel2=0,
                csi_b64='',
                mac_timestamp=0,
            )
            events.append(FTMEvent(
                t_ms=int(parts[1]),
                own_mac=parts[0],
                tgt_mac=parts[3],
                seq=int(parts[2]),
                reports=[report],
            ))
    return events


def load_csi_events_from_mac(filepath, own_mac: str) -> list[CSIEvent]:
    """
    Load CSI CSV into a list of CSIEvent dataclasses, filtered by own_mac.
    CSV columns: t_ms, timestamp, own_mac, tgt_mac, seq, payload_b64, mac_timestamp, rssi
    """
    events = []
    with open(filepath, 'r') as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            parts = line.split(',')
            if len(parts) != 9:
                continue
            if parts[2] != own_mac:
                continue

            events.append(CSIEvent(
                t_ms=int(parts[0]),
                timestamp=int(parts[1]),
                own_mac=parts[2],
                tgt_mac=parts[3],
                seq=int(parts[4]),
                payload_b64=parts[5],
                mac_timestamp=int(parts[6]),
                rssi=int(parts[7]),
                channel=0,
                channel2=0,
                sig_mode=int(parts[8])
            ))
    return events
'''
H : N_pkt x n_subcarrier array of CSI
T : N_pkt x 4 array of timestamps
d_range : CIR will be measured in linspace(d_range[0],d_range[1],d_range[2]) steps
returns:
ranges: Estimated range
h_pk: interpolated CIR at d_range steps
'''
def compute_wipro_range(H,T,d_range=(-260,260,1024), return_CIR=False):

    d_range = np.linspace(d_range[0],d_range[1],d_range[2])
    ffreq = np.fft.fftfreq(128,d=1/40e6)
    DFT_MAT = np.exp((2.0j*np.pi*(d_range[:,None]/3e8)*ffreq[None,:]))
    sinc = np.abs(DFT_MAT @ np.ones((128,)))
    sinc /= np.max(sinc)
    rise_thresh = 0.25
    sinc_rise_comp = - d_range[np.argmax(np.abs(sinc) > rise_thresh)]
    n_step = DFT_MAT.shape[0]
    n_pkt = H.shape[0]
    ranges = np.zeros((n_pkt,))
    h_pks = np.zeros((n_pkt,n_step)) if return_CIR else None
    for ii in range(n_pkt):
        h = H[ii,:]
        t = T[ii,:]
        delay = ((t[3] - t[0]) - (t[2] - t[1])) / 2e12
        h_shift = h*np.exp(-2.0j*np.pi*ffreq*delay)
        h_up = DFT_MAT @ h_shift
        h_pk = np.abs(h_up)
        h_pk /= np.max(h_pk)
        ranges[ii] = d_range[np.argmax(h_pk > rise_thresh)] + sinc_rise_comp
        if return_CIR:
            h_pks[ii,:] = h_pk
    if return_CIR:
        return ranges, h_pks
    else:
        return ranges
        
