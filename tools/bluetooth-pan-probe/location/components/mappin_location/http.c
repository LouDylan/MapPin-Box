#include <string.h>

#include "esp_http_server.h"
#include "lwip/sockets.h"
#include "setup_wifi.h"
#include "mappin_pan.h"

int mappin_location_command(const char *uri, uint32_t local, uint32_t peer,
                            char *reply, size_t capacity);

static const char *page;
static size_t page_size;

static uint32_t socket_ipv4(int fd, bool peer)
{
    struct sockaddr_storage address;
    socklen_t size = sizeof(address);
    struct sockaddr *out = (struct sockaddr *)&address;
    int result = peer ? getpeername(fd, out, &size) : getsockname(fd, out, &size);
    if (result != 0)
        return 0;
    if (address.ss_family == AF_INET)
        return ((const struct sockaddr_in *)&address)->sin_addr.s_addr;
    if (address.ss_family != AF_INET6)
        return 0;
    const struct sockaddr_in6 *v6 = (const struct sockaddr_in6 *)&address;
    if (!IN6_IS_ADDR_V4MAPPED(&v6->sin6_addr))
        return 0;
    uint32_t ip;
    memcpy(&ip, v6->sin6_addr.s6_addr + 12, sizeof(ip));
    return ip;
}

static esp_err_t home(httpd_req_t *request)
{
    int fd = httpd_req_to_sockfd(request);
    uint32_t local = socket_ipv4(fd, false);
    if (!local || (local != mappin_setup_address() &&
        (local != mappin_pan_address() || socket_ipv4(fd, true) != mappin_pan_gateway())))
        return httpd_resp_send_custom_err(request, "403 Forbidden", "Use the setup hotspot or Bluetooth");
    httpd_resp_set_type(request, "text/html; charset=utf-8");
    httpd_resp_set_hdr(request, "Cache-Control", "no-store");
    return httpd_resp_send(request, page, page_size);
}

static bool local_post(httpd_req_t *request)
{
    char header[2];
    return httpd_req_get_hdr_value_str(request, "X-MapPin-Bt-Test", header, sizeof(header)) == ESP_OK &&
           !strcmp(header, "1");
}

static esp_err_t pair_network(httpd_req_t *request)
{
    int fd = httpd_req_to_sockfd(request);
    uint32_t local = socket_ipv4(fd, false);
    if (!local || local != mappin_setup_address())
        return httpd_resp_send_custom_err(request, "403 Forbidden", "Use the setup hotspot");
    if (request->method == HTTP_POST) {
        if (!local_post(request))
            return httpd_resp_send_custom_err(request, "403 Forbidden", "Use the setup page");
        uint8_t bytes[1 + 32 + 63];
        if (request->content_len < 2 || request->content_len > sizeof(bytes))
            return httpd_resp_send_err(request, HTTPD_400_BAD_REQUEST, "Invalid Wi-Fi details");
        size_t offset = 0;
        while (offset < request->content_len) {
            int received = httpd_req_recv(request, (char *)bytes + offset, request->content_len - offset);
            if (received <= 0)
                return ESP_FAIL;
            offset += received;
        }
        if (mappin_setup_join(bytes, offset) != ESP_OK)
            return httpd_resp_send_err(request, HTTPD_400_BAD_REQUEST, "Could not start Wi-Fi connection");
    }
    char reply[96];
    mappin_setup_status(reply, sizeof(reply));
    httpd_resp_set_type(request, "application/json");
    httpd_resp_set_hdr(request, "Cache-Control", "no-store");
    return httpd_resp_sendstr(request, reply);
}

static esp_err_t control(httpd_req_t *request)
{
    int fd = httpd_req_to_sockfd(request);
    uint32_t local = socket_ipv4(fd, false);
    uint32_t peer = socket_ipv4(fd, true);
    bool pan = local == mappin_pan_address() && peer == mappin_pan_gateway();
    bool setup = local == mappin_setup_address() && !strncmp(request->uri, "/pair", 5);
    if (!local || !peer || (!pan && !setup))
        return httpd_resp_send_custom_err(request, "403 Forbidden", "Connect to Bluetooth PAN for location testing");
    if (request->method == HTTP_POST && !local_post(request))
        return httpd_resp_send_custom_err(request, "403 Forbidden", "Use the MapPin test page");

    char reply[512];
    int status = mappin_location_command(request->uri, local, peer, reply, sizeof(reply));
    if (status == 202)
        httpd_resp_set_status(request, "202 Accepted");
    else if (status != 200)
        httpd_resp_set_status(request, "502 Bad Gateway");
    httpd_resp_set_type(request, "text/plain; charset=utf-8");
    httpd_resp_set_hdr(request, "Cache-Control", "no-store");
    return httpd_resp_sendstr(request, reply);
}

esp_err_t mappin_location_http_start(const char *html, size_t size)
{
    page = html;
    page_size = size;
    httpd_handle_t server = NULL;
    httpd_config_t config = HTTPD_DEFAULT_CONFIG();
    config.stack_size = 12 * 1024;
    config.max_open_sockets = 4;
    config.lru_purge_enable = true;
    esp_err_t error = httpd_start(&server, &config);
    if (error != ESP_OK)
        return error;
    const httpd_uri_t routes[] = {
        { .uri = "/", .method = HTTP_GET, .handler = home },
        { .uri = "/pair-status", .method = HTTP_GET, .handler = control },
        { .uri = "/pair", .method = HTTP_POST, .handler = control },
        { .uri = "/pair-network", .method = HTTP_GET, .handler = pair_network },
        { .uri = "/pair-network", .method = HTTP_POST, .handler = pair_network },
        { .uri = "/set", .method = HTTP_POST, .handler = control },
        { .uri = "/restore", .method = HTTP_POST, .handler = control },
    };
    for (size_t i = 0; i < sizeof(routes) / sizeof(routes[0]); i++) {
        error = httpd_register_uri_handler(server, &routes[i]);
        if (error != ESP_OK) {
            httpd_stop(server);
            return error;
        }
    }
    return ESP_OK;
}
