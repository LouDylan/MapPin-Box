idf_component_register(SRCS "http.c" "setup_wifi.c" "pair_trace.c"
                       PRIV_REQUIRES esp_event esp_http_server esp_netif esp_wifi
                                     lwip mappin_pan espressif__mdns)
