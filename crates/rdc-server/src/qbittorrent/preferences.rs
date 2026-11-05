use rdc_engine::Engine;
use serde_json::{Value, json};

use super::models::save_path;

// Dynamic paths and account identity are inserted only after parsing immutable defaults.
const DEFAULTS: &str = r#"{
"add_trackers":"","add_trackers_enabled":false,"alt_dl_limit":10240,"alt_up_limit":10240,
"alternative_webui_enabled":false,"alternative_webui_path":"","announce_ip":"",
"announce_to_all_tiers":true,"announce_to_all_trackers":false,"anonymous_mode":false,
"async_io_threads":4,"auto_delete_mode":0,"auto_tmm_enabled":false,"autorun_enabled":false,
"autorun_program":"","banned_IPs":"","bittorrent_protocol":0,
"bypass_auth_subnet_whitelist":"","bypass_auth_subnet_whitelist_enabled":false,
"bypass_local_auth":false,"category_changed_tmm_enabled":false,"checking_memory_use":32,
"create_subfolder_enabled":true,"current_interface_address":"","current_network_interface":"",
"dht":true,"disk_cache":-1,"disk_cache_ttl":60,"dl_limit":0,"dont_count_slow_torrents":false,
"dyndns_domain":"changeme.dyndns.org","dyndns_enabled":false,"dyndns_password":"",
"dyndns_service":0,"dyndns_username":"","embedded_tracker_port":9000,
"enable_coalesce_read_write":true,"enable_embedded_tracker":false,
"enable_multi_connections_from_same_ip":false,"enable_os_cache":true,
"enable_piece_extent_affinity":false,"enable_super_seeding":false,
"enable_upload_suggestions":false,"encryption":0,"export_dir":"","export_dir_fin":"",
"file_pool_size":40,"incomplete_files_ext":false,"ip_filter_enabled":false,
"ip_filter_path":"","ip_filter_trackers":false,"limit_lan_peers":true,
"limit_tcp_overhead":false,"limit_utp_rate":true,"listen_port":31193,"locale":"en",
"lsd":true,"mail_notification_auth_enabled":false,"mail_notification_email":"",
"mail_notification_enabled":false,"mail_notification_password":"",
"mail_notification_sender":"qBittorrentNotification@example.com",
"mail_notification_smtp":"smtp.changeme.com","mail_notification_ssl_enabled":false,
"mail_notification_username":"","max_active_downloads":3,"max_active_torrents":5,
"max_active_uploads":3,"max_connec":500,"max_connec_per_torrent":100,
"max_ratio":-1,"max_ratio_act":0,"max_ratio_enabled":false,"max_seeding_time":-1,
"max_seeding_time_enabled":false,"max_uploads":-1,"max_uploads_per_torrent":-1,
"outgoing_ports_max":0,"outgoing_ports_min":0,"pex":true,"preallocate_all":false,
"proxy_auth_enabled":false,"proxy_ip":"0.0.0.0","proxy_password":"",
"proxy_peer_connections":false,"proxy_port":8080,"proxy_torrents_only":false,
"proxy_type":0,"proxy_username":"","queueing_enabled":false,"random_port":false,
"recheck_completed_torrents":false,"resolve_peer_countries":true,
"rss_auto_downloading_enabled":false,"rss_max_articles_per_feed":50,
"rss_processing_enabled":false,"rss_refresh_interval":30,
"save_path_changed_tmm_enabled":false,"save_resume_data_interval":60,"scan_dirs":{},
"schedule_from_hour":8,"schedule_from_min":0,"schedule_to_hour":20,"schedule_to_min":0,
"scheduler_days":0,"scheduler_enabled":false,"send_buffer_low_watermark":10,
"send_buffer_watermark":500,"send_buffer_watermark_factor":50,
"slow_torrent_dl_rate_threshold":2,"slow_torrent_inactive_timer":60,
"slow_torrent_ul_rate_threshold":2,"socket_backlog_size":30,
"start_paused_enabled":false,"stop_tracker_timeout":1,"temp_path_enabled":false,
"torrent_changed_tmm_enabled":true,"up_limit":0,"upload_choking_algorithm":1,
"upload_slots_behavior":0,"upnp":true,"upnp_lease_duration":0,"use_https":false,
"utp_tcp_mixed_mode":0,"web_ui_address":"*","web_ui_ban_duration":3600,
"web_ui_clickjacking_protection_enabled":true,"web_ui_csrf_protection_enabled":true,
"web_ui_domain_list":"*","web_ui_host_header_validation_enabled":true,
"web_ui_https_cert_path":"","web_ui_https_key_path":"",
"web_ui_max_auth_fail_count":5,"web_ui_port":8080,"web_ui_secure_cookie_enabled":true,
"web_ui_session_timeout":3600,"web_ui_upnp":false
}"#;

pub(super) fn response(engine: &Engine, username: &str) -> Value {
    let mut value: Value =
        serde_json::from_str(DEFAULTS).expect("constant preference JSON is valid");
    let path = save_path(&engine.settings());
    value["save_path"] = json!(path);
    value["temp_path"] = json!(format!("{path}temp{}", std::path::MAIN_SEPARATOR));
    value["web_ui_username"] = json!(username);
    value
}
