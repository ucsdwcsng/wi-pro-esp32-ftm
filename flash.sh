
#!/bin/bash

set -e

# Run everything in a subshell
(    
    SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
    cd $SCRIPT_DIR/firmware/bin
    if [ $# -eq 0 ]
    then
	echo "Provide dev path as argument"
    else
	esptool --chip esp32s3 --port $1 --baud 460800   --before default_reset --after hard_reset write_flash   0x0 bootloader.bin   0x8000 partition-table.bin   0x10000 firmware.bin
    fi

)

