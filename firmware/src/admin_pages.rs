const SHELL: &str = include_str!("admin.html");

pub const PAGES: &[(&str, &str, &str, &str)] = &[
    (
        "/admin",
        "system",
        "系统信息",
        include_str!("admin/system.html"),
    ),
    (
        "/admin/network",
        "network",
        "Wi-Fi 连接",
        include_str!("admin/network.html"),
    ),
    (
        "/admin/maps",
        "maps",
        "地图服务",
        include_str!("admin/maps.html"),
    ),
    (
        "/admin/location",
        "location",
        "定位调试",
        include_str!("admin/location.html"),
    ),
    (
        "/admin/help",
        "help",
        "使用帮助",
        include_str!("admin/help.html"),
    ),
];

pub fn render(page: &str, title: &str, content: &str) -> String {
    SHELL
        .replace("{{page}}", page)
        .replace("{{title}}", title)
        .replace("{{content}}", content)
}
