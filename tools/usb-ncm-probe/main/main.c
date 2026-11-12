/* USB network probe. No NVS writes, Wi-Fi, pairing or location commands. */
#include <inttypes.h>
#include <stdio.h>

#include "esp_heap_caps.h"
#include "esp_http_server.h"
#include "esp_log.h"
#include "esp_psram.h"
#include "esp_system.h"
#include "esp_timer.h"
#include "lwip/sockets.h"
#include "usb_network.h"

#define PROBE_VERSION "0.1.0"

static const char *TAG = "usb_probe";

extern const char page_start[] asm("_binary_index_html_start");
extern const char page_end[] asm("_binary_index_html_end");

static void response_headers(httpd_req_t *req, const char *type)
{
    httpd_resp_set_type(req, type);
    httpd_resp_set_hdr(req, "Cache-Control", "no-store");
    httpd_resp_set_hdr(req, "X-Content-Type-Options", "nosniff");
}

static esp_err_t home(httpd_req_t *req)
{
    response_headers(req, "text/html; charset=utf-8");
    return httpd_resp_send(req, page_start, page_end - page_start - 1);
}

static void client_address(httpd_req_t *req, char *text, size_t size)
{
    struct sockaddr_storage peer = {0};
    socklen_t peer_len = sizeof(peer);
    if (getpeername(httpd_req_to_sockfd(req), (struct sockaddr *)&peer, &peer_len) != 0)
        return;
    if (peer.ss_family == AF_INET) {
        inet_ntop(AF_INET, &((struct sockaddr_in *)&peer)->sin_addr, text, size);
        return;
    }
    if (peer.ss_family != AF_INET6)
        return;
    const struct in6_addr *ip = &((struct sockaddr_in6 *)&peer)->sin6_addr;
    if (IN6_IS_ADDR_V4MAPPED(ip))
        inet_ntop(AF_INET, &ip->s6_addr[12], text, size);
    else
        inet_ntop(AF_INET6, ip, text, size);
}

static esp_err_t status_page(httpd_req_t *req)
{
    char client[INET6_ADDRSTRLEN] = "";
    client_address(req, client, sizeof(client));
    struct mappin_usb_network_status status;
    mappin_usb_network_status(&status);
    char body[768];
    int len = snprintf(body, sizeof(body),
        "{\"version\":\"" PROBE_VERSION "\",\"address\":\"" MAPPIN_USB_ADDRESS "\","
        "\"client\":\"%s\",\"attached\":%s,\"suspended\":%s,"
        "\"mounts\":%" PRIu32 ",\"resumes\":%" PRIu32 ","
        "\"rx_packets\":%" PRIu32 ",\"tx_packets\":%" PRIu32 ","
        "\"rx_dropped\":%" PRIu32 ",\"tx_dropped\":%" PRIu32 ","
        "\"uptime_s\":%" PRId64 ",\"heap\":%u,\"internal_heap\":%u,"
        "\"psram\":%u,\"reset_reason\":%d,"
        "\"dhcp_router\":false,\"dhcp_dns\":false,\"location_supported\":false}",
        client, status.attached ? "true" : "false",
        status.suspended ? "true" : "false",
        status.mounts, status.resumes,
        status.rx_packets, status.tx_packets,
        status.rx_dropped, status.tx_dropped,
        esp_timer_get_time() / 1000000, (unsigned)esp_get_free_heap_size(),
        (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT),
        (unsigned)esp_psram_get_size(), esp_reset_reason());
    if (len < 0 || len >= sizeof(body))
        return httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR, "status overflow");
    response_headers(req, "application/json");
    return httpd_resp_send(req, body, len);
}

static void start_http(void)
{
    httpd_config_t config = HTTPD_DEFAULT_CONFIG();
    config.stack_size = 6144;
    config.max_open_sockets = 4;
    config.lru_purge_enable = true;
    httpd_handle_t server;
    ESP_ERROR_CHECK(httpd_start(&server, &config));
    const httpd_uri_t routes[] = {
        {.uri = "/", .method = HTTP_GET, .handler = home},
        {.uri = "/api/status", .method = HTTP_GET, .handler = status_page},
    };
    for (size_t i = 0; i < sizeof(routes) / sizeof(routes[0]); i++)
        ESP_ERROR_CHECK(httpd_register_uri_handler(server, &routes[i]));
}

void app_main(void)
{
    ESP_LOGI(TAG, "MapPin-Box USB-NCM probe " PROBE_VERSION "; no persistent writes");
    mappin_usb_network_start();
    start_http();
    ESP_LOGI(TAG, "MAPPIN_USB_NCM_READY http://" MAPPIN_USB_ADDRESS " Router=0 DNS=0");
}
