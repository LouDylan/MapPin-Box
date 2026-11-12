#pragma once

#include <stdint.h>

#define MAPPIN_USB_ADDRESS "192.168.72.1"

struct mappin_usb_network_status {
    uint32_t attached;
    uint32_t suspended;
    uint32_t mounts;
    uint32_t resumes;
    uint32_t rx_packets;
    uint32_t tx_packets;
    uint32_t rx_dropped;
    uint32_t tx_dropped;
};

void mappin_usb_network_start(void);
void mappin_usb_network_status(struct mappin_usb_network_status *status);
