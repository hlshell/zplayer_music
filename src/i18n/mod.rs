//! 语言加载（i18n）：内置 `ui/assets/lang/zh.json`、`en.json`（编译期嵌入）为兜底，
//! 扫描**二进制同目录 `lang/*.json`** 作为用户扩展；按当前语言短码合并后写入
//! `Language` global（扁平 key → 属性，见 ui/language.slint）。
//!
//! 合并规则：中文 zh 为默认兜底 → 目标语言（内置或用户扩展）覆盖同 key → 缺失 key 回退中文。
//! 用户扩展语言显示名取自 json 顶层可选 `meta.name`，缺省用大写短码。

use slint::ComponentHandle;
use std::collections::HashMap;

const ZH_JSON: &str = include_str!("../../ui/assets/lang/zh.json");
const EN_JSON: &str = include_str!("../../ui/assets/lang/en.json");

/// 拍平 json 顶层字符串键值；预留嵌套（key 用 "a.b"）暂不需要。
fn flatten(v: &serde_json::Value) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            if let serde_json::Value::String(s) = val {
                m.insert(k.clone(), s.clone());
            }
        }
    }
    m
}

/// 二进制同目录（exe 所在）`lang/*.json`，返回 短码 → (显示名, Value)
fn scan_user_langs() -> HashMap<String, (String, serde_json::Value)> {
    let mut out = HashMap::new();
    let Some(exe) = std::env::current_exe().ok() else { return out };
    let dir = exe.parent().map(|p| p.join("lang")).unwrap_or_default();
    if !dir.is_dir() {
        return out;
    }
    let Ok(entries) = std::fs::read_dir(&dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let code = match p.file_stem().and_then(|s| s.to_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => continue,
        };
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let name = val
            .get("meta")
            .and_then(|m| m.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| code.to_uppercase());
        out.insert(code, (name, val));
    }
    out
}

/// 可用语言列表（内置 zh/en + 用户扩展），返回 (code, 显示名)。
pub fn list_languages() -> Vec<(String, String)> {
    let mut v = vec![
        ("zh".to_string(), "简体中文".to_string()),
        ("en".to_string(), "English".to_string()),
    ];
    for (code, (name, _)) in scan_user_langs() {
        if code != "zh" && code != "en" {
            v.push((code, name));
        }
    }
    v
}

/// 按短码加载语言并写入 `Language` global。
/// 找不到目标语言时回退中文（zh 兜底）。
pub fn load(ui: &crate::AppWindow, code: &str) {
    let zh: HashMap<String, String> = flatten(&serde_json::from_str(ZH_JSON).unwrap_or_default());
    let mut map = zh.clone(); // 中文兜底
    // 内置英文
    if code == "en" {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(EN_JSON) {
            map.extend(flatten(&v));
        }
    }
    // 用户扩展覆盖
    if let Some((_, v)) = scan_user_langs().get(code) {
        map.extend(flatten(&v));
    }
    apply(ui, &map);
}

/// 把扁平 key 逐一写入 Language global（key 与属性名一一对应）。
fn apply(ui: &crate::AppWindow, map: &HashMap<String, String>) {
    let l = ui.global::<crate::Language>();
    let g = |k: &str| -> slint::SharedString { map.get(k).cloned().unwrap_or_default().into() };
    // 通用
    l.set_common_ok(g("common-ok"));
    l.set_common_close(g("common-close"));
    l.set_common_cancel(g("common-cancel"));
    l.set_common_check_update(g("common-check-update"));
    l.set_common_current_version(g("common-current-version"));
    l.set_common_author_info(g("common-author-info"));
    // 导航
    l.set_nav_home(g("nav-home"));
    l.set_nav_search(g("nav-search"));
    l.set_nav_playlists(g("nav-playlists"));
    l.set_nav_albums(g("nav-albums"));
    l.set_nav_mv(g("nav-mv"));
    l.set_nav_radio(g("nav-radio"));
    l.set_nav_toplist(g("nav-toplist"));
    l.set_nav_liked(g("nav-liked"));
    l.set_nav_recent(g("nav-recent"));
    l.set_nav_local(g("nav-local"));
    l.set_nav_settings(g("nav-settings"));
    l.set_nav_not_logged(g("nav-not-logged"));
    l.set_nav_login_hint(g("nav-login-hint"));
    // Tab
    l.set_tab_basic(g("tab-basic"));
    l.set_tab_play(g("tab-play"));
    l.set_tab_app(g("tab-app"));
    l.set_tab_network(g("tab-network"));
    l.set_tab_system(g("tab-system"));
    l.set_tab_about(g("tab-about"));
    l.set_tab_donate(g("tab-donate"));
    // 基础
    l.set_base_theme_mode(g("base-theme-mode"));
    l.set_base_theme_mode_desc(g("base-theme-mode-desc"));
    l.set_base_light(g("base-light"));
    l.set_base_dark(g("base-dark"));
    l.set_base_auto_launch(g("base-auto-launch"));
    l.set_base_auto_launch_desc(g("base-auto-launch-desc"));
    l.set_base_language(g("base-language"));
    l.set_base_language_desc(g("base-language-desc"));
    l.set_base_font(g("base-font"));
    l.set_base_font_desc(g("base-font-desc"));
    l.set_base_font_default(g("base-font-default"));
    l.set_base_font_yahei(g("base-font-yahei"));
    l.set_base_font_sourcehan(g("base-font-sourcehan"));
    // 播放
    l.set_play_quality(g("play-quality"));
    l.set_play_quality_desc(g("play-quality-desc"));
    l.set_play_q_standard(g("play-q-standard"));
    l.set_play_q_high(g("play-q-high"));
    l.set_play_q_lossless(g("play-q-lossless"));
    l.set_play_source(g("play-source"));
    l.set_play_source_desc(g("play-source-desc"));
    l.set_play_view(g("play-view"));
    l.set_play_auto_play(g("play-auto-play"));
    l.set_play_auto_play_desc(g("play-auto-play-desc"));
    l.set_play_output_device(g("play-output-device"));
    l.set_play_output_desc(g("play-output-desc"));
    l.set_play_script_title(g("play-script-title"));
    l.set_play_import_script(g("play-import-script"));
    l.set_play_import_desc(g("play-import-desc"));
    l.set_play_select_file(g("play-select-file"));
    l.set_play_check_update_script(g("play-check-update-script"));
    l.set_play_check_update_desc(g("play-check-update-desc"));
    l.set_play_auto_update_script(g("play-auto-update-script"));
    l.set_play_auto_update_desc(g("play-auto-update-desc"));
    l.set_play_lx_fallback(g("play-lx-fallback"));
    l.set_play_lx_fallback_desc(g("play-lx-fallback-desc"));
    l.set_play_no_scripts(g("play-no-scripts"));
    // 应用
    l.set_app_theme_color(g("app-theme-color"));
    l.set_app_theme_color_desc(g("app-theme-color-desc"));
    l.set_app_c_purple(g("app-c-purple"));
    l.set_app_c_teal(g("app-c-teal"));
    l.set_app_c_orange(g("app-c-orange"));
    l.set_app_c_blue(g("app-c-blue"));
    l.set_app_lyrics_layout(g("app-lyrics-layout"));
    l.set_app_lyrics_desc(g("app-lyrics-desc"));
    l.set_app_lyrics_cover(g("app-lyrics-cover"));
    l.set_app_lyrics_side(g("app-lyrics-side"));
    l.set_app_close_behavior(g("app-close-behavior"));
    l.set_app_close_desc(g("app-close-desc"));
    l.set_app_close_ask(g("app-close-ask"));
    l.set_app_close_tray(g("app-close-tray"));
    l.set_app_close_exit(g("app-close-exit"));
    l.set_app_shortcut(g("app-shortcut"));
    l.set_app_shortcut_desc(g("app-shortcut-desc"));
    l.set_app_shortcut_custom(g("app-shortcut-custom"));
    // 网络
    l.set_net_threads(g("net-threads"));
    l.set_net_threads_desc(g("net-threads-desc"));
    l.set_net_buffer_note(g("net-buffer-note"));
    l.set_net_proxy(g("net-proxy"));
    l.set_net_proxy_desc(g("net-proxy-desc"));
    l.set_net_configure(g("net-configure"));
    l.set_net_realip(g("net-realip"));
    l.set_net_realip_desc(g("net-realip-desc"));
    l.set_net_realip_placeholder(g("net-realip-placeholder"));
    l.set_net_proxy_addr(g("net-proxy-addr"));
    l.set_net_proxy_port(g("net-proxy-port"));
    l.set_net_proxy_host_ph(g("net-proxy-host-ph"));
    l.set_net_proxy_port_ph(g("net-proxy-port-ph"));
    l.set_proxy_title(g("proxy-title"));
    l.set_proxy_type(g("proxy-type"));
    // 系统
    l.set_sys_cache(g("sys-cache"));
    l.set_sys_cache_desc(g("sys-cache-desc"));
    l.set_sys_cache_dir(g("sys-cache-dir"));
    l.set_sys_cache_dir_desc(g("sys-cache-dir-desc"));
    l.set_sys_cache_limit(g("sys-cache-limit"));
    l.set_sys_cache_limit_desc(g("sys-cache-limit-desc"));
    l.set_sys_mb(g("sys-mb"));
    l.set_sys_evict(g("sys-evict"));
    l.set_sys_evict_desc(g("sys-evict-desc"));
    l.set_sys_evict_lru(g("sys-evict-lru"));
    l.set_sys_evict_fifo(g("sys-evict-fifo"));
    l.set_sys_status_title(g("sys-status-title"));
    l.set_sys_manage_title(g("sys-manage-title"));
    l.set_sys_clear_music(g("sys-clear-music"));
    l.set_sys_clear_lyrics(g("sys-clear-lyrics"));
    l.set_sys_clear_covers(g("sys-clear-covers"));
    l.set_sys_clear_all(g("sys-clear-all"));
    l.set_sys_music_count(g("sys-music-count"));
    l.set_sys_lyric_count(g("sys-lyric-count"));
    l.set_sys_cover_count(g("sys-cover-count"));
    l.set_sys_unit_song(g("sys-unit-song"));
    l.set_sys_unit_cover(g("sys-unit-cover"));
    // 关于
    l.set_about_author_value(g("about-author-value"));
    l.set_about_tagline(g("about-tagline"));
    // 捐赠
    l.set_donate_intro(g("donate-intro"));
    l.set_donate_purpose(g("donate-purpose"));
    l.set_donate_button(g("donate-button"));
    // 音源弹窗
    l.set_sources_title(g("sources-title"));
    l.set_sources_connected(g("sources-connected"));
    l.set_sources_pending(g("sources-pending"));
    l.set_sources_netease(g("sources-netease"));
    l.set_sources_soundcloud(g("sources-soundcloud"));
    l.set_sources_qq(g("sources-qq"));
    l.set_sources_kugou(g("sources-kugou"));
    l.set_sources_kuwo(g("sources-kuwo"));
    l.set_sources_migu(g("sources-migu"));
    l.set_sources_youtube(g("sources-youtube"));
    // 详情 / 卡片
    l.set_detail_play_all(g("detail-play-all"));
    l.set_detail_favorite(g("detail-favorite"));
    l.set_cards_more(g("cards-more"));
    l.set_cards_daily_title(g("cards-daily-title"));
    l.set_cards_count_songs(g("cards-count-songs"));
    // 通用组件
    l.set_loading_tip(g("loading-tip"));
    l.set_lyrics_cover_mode(g("lyrics-cover-mode"));
    l.set_lyrics_split_mode(g("lyrics-split-mode"));
    l.set_lyrics_immersive_mode(g("lyrics-immersive-mode"));
    l.set_mini_not_playing(g("mini-not-playing"));
    l.set_playlist_title(g("playlist-title"));
    l.set_songlist_col_title(g("songlist-col-title"));
    l.set_songlist_col_duration(g("songlist-col-duration"));
    l.set_suggest_kind_song(g("suggest-kind-song"));
    l.set_suggest_kind_artist(g("suggest-kind-artist"));
    l.set_suggest_kind_album(g("suggest-kind-album"));
    l.set_suggest_kind_playlist(g("suggest-kind-playlist"));
    l.set_suggest_kind_lyric(g("suggest-kind-lyric"));
    l.set_suggest_kind_radio(g("suggest-kind-radio"));
    l.set_suggest_kind_mixed(g("suggest-kind-mixed"));
    l.set_titlebar_search_placeholder(g("titlebar-search-placeholder"));
    l.set_titlebar_search(g("titlebar-search"));
    // 详情页
    l.set_detail_search_song(g("detail-search-song"));
    l.set_detail_song_unit(g("detail-song-unit"));
    // 首页
    l.set_home_curated(g("home-curated"));
    l.set_home_curated_desc(g("home-curated-desc"));
    l.set_home_curated_update(g("home-curated-update"));
    l.set_home_trending(g("home-trending"));
    l.set_home_hot_playlists(g("home-hot-playlists"));
    l.set_home_artists(g("home-artists"));
    l.set_home_new_releases(g("home-new-releases"));
    // 搜索页
    l.set_search_hot(g("search-hot"));
    l.set_search_hot_list(g("search-hot-list"));
    l.set_search_hot_list_desc(g("search-hot-list-desc"));
    l.set_search_history(g("search-history"));
    l.set_search_history_clear(g("search-history-clear"));
    l.set_search_back(g("search-back"));
    l.set_search_result_prefix(g("search-result-prefix"));
    l.set_search_result_suffix(g("search-result-suffix"));
    l.set_search_empty_title(g("search-empty-title"));
    l.set_search_empty_desc(g("search-empty-desc"));
    // 网格页
    l.set_page_all_albums(g("page-all-albums"));
    l.set_page_all_playlists(g("page-all-playlists"));
    l.set_page_all_mv(g("page-all-mv"));
    l.set_page_all_radio(g("page-all-radio"));
    l.set_page_sub_albums(g("page-sub-albums"));
    l.set_page_sub_playlists(g("page-sub-playlists"));
    l.set_page_sub_mv(g("page-sub-mv"));
    l.set_page_sub_radio(g("page-sub-radio"));
    l.set_page_notready_title(g("page-notready-title"));
    l.set_page_notready_desc(g("page-notready-desc"));
    l.set_page_no_more(g("page-no-more"));
    // 本地音乐
    l.set_local_stat_song(g("local-stat-song"));
    l.set_local_stat_album(g("local-stat-album"));
    l.set_local_stat_artist(g("local-stat-artist"));
    l.set_local_stat_total(g("local-stat-total"));
    l.set_local_empty_title_web(g("local-empty-title-web"));
    l.set_local_empty_desc_web(g("local-empty-desc-web"));
    l.set_local_empty_title(g("local-empty-title"));
    l.set_local_empty_desc(g("local-empty-desc"));
    // 我的最爱
    l.set_liked_count_songs(g("liked-count-songs"));
    l.set_liked_count_favs(g("liked-count-favs"));
    l.set_liked_count_tail(g("liked-count-tail"));
    l.set_liked_kind_other(g("liked-kind-other"));
    l.set_liked_empty_song_title(g("liked-empty-song-title"));
    l.set_liked_empty_song_desc(g("liked-empty-song-desc"));
    l.set_liked_empty_playlist_title(g("liked-empty-playlist-title"));
    l.set_liked_empty_playlist_desc(g("liked-empty-playlist-desc"));
    l.set_liked_empty_album_title(g("liked-empty-album-title"));
    l.set_liked_empty_album_desc(g("liked-empty-album-desc"));
    l.set_liked_empty_mv_title(g("liked-empty-mv-title"));
    l.set_liked_empty_mv_desc(g("liked-empty-mv-desc"));
    l.set_liked_empty_radio_title(g("liked-empty-radio-title"));
    l.set_liked_empty_radio_desc(g("liked-empty-radio-desc"));
    // 最近播放
    l.set_recent_count_prefix(g("recent-count-prefix"));
    l.set_recent_count_unit(g("recent-count-unit"));
    l.set_recent_clear(g("recent-clear"));
    l.set_recent_empty_title(g("recent-empty-title"));
    l.set_recent_empty_desc(g("recent-empty-desc"));
    // 排行榜
    l.set_toplist_official(g("toplist-official"));
    l.set_toplist_global(g("toplist-global"));
    l.set_toplist_update_live(g("toplist-update-live"));
    l.set_toplist_update_daily(g("toplist-update-daily"));
    l.set_toplist_update_weekly(g("toplist-update-weekly"));
    l.set_toplist_name_1(g("toplist-name-1"));
    l.set_toplist_name_2(g("toplist-name-2"));
    l.set_toplist_name_3(g("toplist-name-3"));
    l.set_toplist_name_4(g("toplist-name-4"));
    l.set_toplist_name_5(g("toplist-name-5"));
    l.set_toplist_name_6(g("toplist-name-6"));
    // 占位页
    l.set_placeholder_title(g("placeholder-title"));
    l.set_placeholder_desc(g("placeholder-desc"));
}

// ---- 用户扩展语言 单测：扫描 + meta.name + 合并兜底语义 ----
#[cfg(test)]
mod tests {
    use super::*;

    fn lang_dir() -> std::path::PathBuf {
        std::env::current_exe().unwrap().parent().unwrap().join("lang")
    }

    #[test]
    fn user_lang_scan_and_meta_name() {
        let d = lang_dir();
        std::fs::create_dir_all(&d).unwrap();
        let _ = std::fs::remove_file(d.join("fr.json"));
        std::fs::write(
            d.join("fr.json"),
            r#"{ "meta": { "name": "Français" }, "nav-home": "Accueil", "base-language": "Langue" }"#,
        )
        .unwrap();

        let langs = list_languages();
        let fr = langs.iter().find(|(c, _)| c == "fr");
        assert!(fr.is_some(), "fr 用户扩展应出现在语言列表");
        assert_eq!(fr.unwrap().1, "Français", "显示名应取 meta.name");

        // 合并兜底语义：fr 只含部分 key，缺失 key 由 zh 兜底，命中 key 被覆盖
        let zh = flatten(&serde_json::from_str(ZH_JSON).unwrap());
        let mut map = zh.clone();
        if let Some((_, v)) = scan_user_langs().get("fr") {
            map.extend(flatten(v));
        }
        assert_eq!(map.get("nav-home").map(|s| s.as_str()), Some("Accueil"));
        assert_eq!(map.get("base-language").map(|s| s.as_str()), Some("Langue"));
        // nav-search 在 fr.json 未提供 → 保持中文兜底
        assert_eq!(map.get("nav-search").map(|s| s.as_str()), Some("搜索"));

        let _ = std::fs::remove_file(d.join("fr.json"));
    }
}
