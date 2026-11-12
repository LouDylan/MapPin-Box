idf_component_register(SRCS "main.c"
                       EMBED_TXTFILES "index.html"
                       PRIV_REQUIRES waypin_usb_network esp_http_server esp_timer esp_psram)
