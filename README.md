# wi-pro-esp32-ftm

*Accurate Range+CSI on the ESP32 using Multipath Compensation*


![Wi-PRO is a system for getting high-accuracy, low-bias indoor range estimation on the ESP32.](images/banner.png)

Wi-PRO (Wireless Positioning with Range Offsets) is a system for getting high-accuracy, low-bias indoor range estimation on the ESP32. Wi-PRO uses a specialized multipath rejection algorithm, currently undergoing publication in IEEE DySPAN '26, to obtain high-accuracy indoor range estimation. Wi-PRO obtains meter-level accuracy in indoor scenarios with dense multipath, while the ESP32's FTM implementation can be off by by tens of meters.

This repository contains everything needed to develop a robust, meter-level-accurate localization system with ESP32 nodes. Wi-PRO nodes automatically discover eachother and initiate FTM ranging requests, and locally run Wi-PRO's algorithm to remove multipath-induced biases and extract an accurate range. FTM+CSI data can also be offloaded to a remote host machine for storage or for multi-node fusion. Wi-PRO can also collect CSI from arbitrary transmitters in monitor mode, making it a good platform for other Wi-Fi sensing applications like traffic monitoring and motion detection.

Currently, Wi-PRO supports the ESP32-S3, but we are actively working on support for the ESP32-C5.

# Building

The firmware is written in rust using `esp-idf-sys` with embassy. The `controller` is a separate rust project.

1. Install rust: https://rust-lang.org/tools/install/

2. Install esp-rs prereqs

```
sudo apt-get install git wget flex bison gperf python3 python3-pip python3-venv cmake ninja-build ccache libffi-dev libssl-dev dfu-util libusb-1.0-0
pip3 install esptool

cargo install cargo-generate
cargo install ldproxy
cargo install espup
cargo install espflash
cargo install cargo-espflash # Optional

espup install

```

## Quick Start

```
./build.sh # build everything
./flash.sh /dev/ttyACM0  # flash the firmware to the ESP (replace /dev/ttyACM0 with ESP's device file)
./controller/controller -p /dev/ttyACM0 -o ./data # connect to the ESP, save FTM/CSI data to ./data
```

## Building and flashing the firmware

```
./build_firmware.sh
```

This should install firmware binaries and object files for debugging in `firmware/bin`. Once you have built, you can run.

```
./flash.sh /dev/ttyACM0 #fill in USB dev for your machine
```

to download the firmware to the esp32

On linux the esp32-s3 will usually be assigned `/dev/ttyACM0`, on MacOS it usually gets assigned to `/dev/cu.usbmodem0`

`build_firmware.sh` which runs `cargo build`, plus a few additional project-specific steps:

-  Run `scripts/create_idft_mat.py` to create the IDFT matrix binary data used to do the up-sampling IDFT, the matrices are compressed with SVD and baked into firmware image themselves to save on RAM, so they need to be pre-generated.

-  Install patches to `esp-idf` to expose the raw mac timestamp in CSI callback. `cargo build` attempts to download and build all of `esp-idf` internally on the first build, so the first time you run the script, it will build esp-idf-sys twice (run cargo build to download esp-idf, then patch esp-idf, then rerun to build with the patch.)

`build_firmware.sh` does all of this and then copies the final binaries into `firmware/bin`.

## Building the controller and connecting to the ESP32

```
./build_controller.sh
```

The ESP32 dumps raw CSI and FTM data over the USB, so we have built a small host binary `controller` which decodes this data and stores it in `csv` files for easy processing. It can also send data to a remote server via ZMQ, for aggregation of data from multiple ESPs together.

Once the ESP32 is flashed, you can connect to it by running

```
./controller/controller -p /dev/ttyACM0
```

Optionally, add `-o data` to save outputs in a folder called `data`. If the folder is non-empty, controller will create a unique subfolder to ensure you don't overwrite existing log files.

# CLI commands

The Wi-PRO firmware exposes a basic command line interface to be controlled over USB serial. `controller` exposes this interface as well. Once you flash and connect to the ESP, you can type `help` in the console to print a list of commands.

## Commands

`help` -- Prints a list of commands

`peers` -- lists all discovered peers. Each node runs a softAP with a fixed SSID (configurable in `firmware/src/config.rs`.) The firmware will keep running Wi-Fi scans until it detects at least one other BSS matching its SSID.

`stats` -- print number of sent/received FTM exchanges

`id` -- print the node's MAC address

`mute <1|0>` -- Enable/disable sending FTM exchanges. All nodes start with `mute 1` by default.

`beacon <ms>` -- Send empty broadcast frames every `<ms>` milliseconds. This can be used for very high-rate CSI collection, for example for motion-detection. Set to 0 to disable. You may need to enable promiscuous mode at the other end to receive CSI from beacon packets.

`interval <ms>` -- Set the rate at which FTM exchanges run. The ESP will attempt to set an FTM exchange to every known peer every `<ms>` milliseconds if mute is set to 0. The default value is `5000`.

`promi <1|0>` -- Enable/disable promiscuous mode. In promiscuous mode, the ESP will report a CSI for every frame it receives. By default, we filter promiscuous packets to only extract data frames, if you want control packets as well, you can modify `set_promi` in `wifi.rs`

`channel <ch>/<bw>` -- Set channel and bandwidth of the FTM responder/beacon packets.

`burst <16|25|32|64>` -- Total number of FTM packets to send per exchange. The hardware sends FTMs in 8-packet bursts every 100ms, you can run 2-8 bursts (so 16-64 total packets.)
