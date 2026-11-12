#include <stdatomic.h>

#include "esp_log.h"
#include "lwip/ip.h"
#include "lwip/raw.h"
#include "lwip/tcpip.h"

static atomic_uint listen_port;

/* Observe only initial connection packets for the active pairing port. */
static u8_t on_tcp(void *arg, struct raw_pcb *pcb, struct pbuf *packet, const ip_addr_t *source)
{
    unsigned port = atomic_load(&listen_port);
    uint8_t header[14];
    if (!port || pbuf_copy_partial(packet, header, sizeof(header), ip_current_header_tot_len()) != sizeof(header))
        return 0;
    unsigned destination = (header[2] << 8) | header[3];
    if (destination == port && (header[13] & 0x12) == 0x02)
        ESP_LOGI("pair_trace", "MAPPIN_PAIR_SYN IPv%s", IP_IS_V6(source) ? "6" : "4");
    return 0;
}

static void setup_trace(void *arg)
{
    static struct raw_pcb *pcb;
    if (pcb)
        return;
    pcb = raw_new_ip_type(IPADDR_TYPE_ANY, IP_PROTO_TCP);
    if (pcb)
        raw_recv(pcb, on_tcp, NULL);
}

void mappin_pair_trace(uint16_t port)
{
    atomic_store(&listen_port, port);
    if (port)
        tcpip_callback(setup_trace, NULL);
}
