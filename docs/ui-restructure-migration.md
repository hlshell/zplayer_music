# ui/ 目录重构迁移清单（平台优先）

> 本清单由脚本按真实 import / 资源引用计算生成，仅用于评审，**尚未修改任何文件**。

## 目标结构

```
ui/
├── shared/            theme.slint  language.slint  models.slint
├── desktop/           appwindow.slint  appwindow.web.slint(薄 wrapper)
│   ├── components/    原 ui/components（models.slint 除外）
│   └── pages/         原 ui/pages
├── mobile/            路径不变，仅 import 改为引用 shared
└── assets/            位置不变（Rust include_str/include_bytes 依赖）
```

## 迁移映射（共 71 个文件，其中 67 个需改引用）

| 旧路径 | 新路径 | 改动行数 | git 状态 |
|---|---|---|---|
| `ui/appwindow-web.slint` | `ui/desktop/appwindow.web.slint` | 1 |  |
| `ui/appwindow.slint` | `ui/desktop/appwindow.slint` | 4 | **有未提交改动** |
| `ui/components/cards.slint` | `ui/desktop/components/cards.slint` | 9 |  |
| `ui/components/close-dialog.slint` | `ui/desktop/components/close-dialog.slint` | 2 |  |
| `ui/components/desktop-lyrics.slint` | `ui/desktop/components/desktop-lyrics.slint` | 2 | 未跟踪 |
| `ui/components/detail.slint` | `ui/desktop/components/detail.slint` | 4 |  |
| `ui/components/loading.slint` | `ui/desktop/components/loading.slint` | 2 |  |
| `ui/components/lyrics-ink.slint` | `ui/desktop/components/lyrics-ink.slint` | 0 | 未跟踪 |
| `ui/components/lyrics.slint` | `ui/desktop/components/lyrics.slint` | 8 | **有未提交改动** |
| `ui/components/marquee.slint` | `ui/desktop/components/marquee.slint` | 1 |  |
| `ui/components/media.slint` | `ui/desktop/components/media.slint` | 5 | **有未提交改动** |
| `ui/components/mini-player.slint` | `ui/desktop/components/mini-player.slint` | 18 |  |
| `ui/components/models.slint` | `ui/shared/models.slint` | 0 |  |
| `ui/components/nav.slint` | `ui/desktop/components/nav.slint` | 13 |  |
| `ui/components/playbar.slint` | `ui/desktop/components/playbar.slint` | 20 | **有未提交改动** |
| `ui/components/playlist.slint` | `ui/desktop/components/playlist.slint` | 8 | **有未提交改动** |
| `ui/components/progressbar.slint` | `ui/desktop/components/progressbar.slint` | 1 |  |
| `ui/components/scroll.slint` | `ui/desktop/components/scroll.slint` | 1 |  |
| `ui/components/scrollable-text-input.slint` | `ui/desktop/components/scrollable-text-input.slint` | 1 |  |
| `ui/components/song-menu.slint` | `ui/desktop/components/song-menu.slint` | 7 | 未跟踪 |
| `ui/components/songlist.slint` | `ui/desktop/components/songlist.slint` | 4 | **有未提交改动** |
| `ui/components/splash.slint` | `ui/desktop/components/splash.slint` | 2 |  |
| `ui/components/suggest.slint` | `ui/desktop/components/suggest.slint` | 4 |  |
| `ui/components/tab-bar.slint` | `ui/desktop/components/tab-bar.slint` | 1 |  |
| `ui/components/titlebar.slint` | `ui/desktop/components/titlebar.slint` | 13 |  |
| `ui/components/toast.slint` | `ui/desktop/components/toast.slint` | 1 |  |
| `ui/components/tray-menu.slint` | `ui/desktop/components/tray-menu.slint` | 3 | **有未提交改动** |
| `ui/components/update-dialog.slint` | `ui/desktop/components/update-dialog.slint` | 2 |  |
| `ui/language.slint` | `ui/shared/language.slint` | 0 | **有未提交改动** |
| `ui/mobile/appwindow-mobile.slint` | `ui/mobile/appwindow-mobile.slint` | 2 |  |
| `ui/mobile/components/glass.slint` | `ui/mobile/components/glass.slint` | 1 |  |
| `ui/mobile/components/mini-player.slint` | `ui/mobile/components/mini-player.slint` | 1 |  |
| `ui/mobile/components/nav.slint` | `ui/mobile/components/nav.slint` | 1 |  |
| `ui/mobile/components/playbar.slint` | `ui/mobile/components/playbar.slint` | 1 |  |
| `ui/mobile/components/song-row.slint` | `ui/mobile/components/song-row.slint` | 1 |  |
| `ui/mobile/pages/detail.slint` | `ui/mobile/pages/detail.slint` | 2 |  |
| `ui/mobile/pages/home.slint` | `ui/mobile/pages/home.slint` | 2 |  |
| `ui/mobile/pages/player.slint` | `ui/mobile/pages/player.slint` | 1 |  |
| `ui/mobile/pages/search.slint` | `ui/mobile/pages/search.slint` | 2 |  |
| `ui/mobile/pages/settings.slint` | `ui/mobile/pages/settings.slint` | 1 |  |
| `ui/mobile/theme.slint` | `ui/mobile/theme.slint` | 1 |  |
| `ui/pages/albums/albums.slint` | `ui/desktop/pages/albums/albums.slint` | 3 |  |
| `ui/pages/detail.slint` | `ui/desktop/pages/detail.slint` | 19 | **有未提交改动** |
| `ui/pages/home/home.slint` | `ui/desktop/pages/home/home.slint` | 5 | **有未提交改动** |
| `ui/pages/liked/liked.slint` | `ui/desktop/pages/liked/liked.slint` | 5 |  |
| `ui/pages/local/local.slint` | `ui/desktop/pages/local/local.slint` | 6 |  |
| `ui/pages/mv/mv.slint` | `ui/desktop/pages/mv/mv.slint` | 3 |  |
| `ui/pages/mv_player.slint` | `ui/desktop/pages/mv_player.slint` | 30 |  |
| `ui/pages/placeholder/placeholder.slint` | `ui/desktop/pages/placeholder/placeholder.slint` | 2 |  |
| `ui/pages/playlists/playlists.slint` | `ui/desktop/pages/playlists/playlists.slint` | 3 |  |
| `ui/pages/radio/radio.slint` | `ui/desktop/pages/radio/radio.slint` | 3 |  |
| `ui/pages/recent/recent.slint` | `ui/desktop/pages/recent/recent.slint` | 6 | **有未提交改动** |
| `ui/pages/search/search.slint` | `ui/desktop/pages/search/search.slint` | 5 |  |
| `ui/pages/settings/about_tab.slint` | `ui/desktop/pages/settings/about_tab.slint` | 4 |  |
| `ui/pages/settings/app_tab.slint` | `ui/desktop/pages/settings/app_tab.slint` | 9 | **有未提交改动** |
| `ui/pages/settings/base_tab.slint` | `ui/desktop/pages/settings/base_tab.slint` | 6 | **有未提交改动** |
| `ui/pages/settings/donate_tab.slint` | `ui/desktop/pages/settings/donate_tab.slint` | 3 |  |
| `ui/pages/settings/network_tab.slint` | `ui/desktop/pages/settings/network_tab.slint` | 5 |  |
| `ui/pages/settings/play_tab.slint` | `ui/desktop/pages/settings/play_tab.slint` | 10 | **有未提交改动** |
| `ui/pages/settings/proxy-popup.slint` | `ui/desktop/pages/settings/proxy-popup.slint` | 2 |  |
| `ui/pages/settings/select-popup.slint` | `ui/desktop/pages/settings/select-popup.slint` | 2 |  |
| `ui/pages/settings/setting-input-row.slint` | `ui/desktop/pages/settings/setting-input-row.slint` | 1 |  |
| `ui/pages/settings/setting-row.slint` | `ui/desktop/pages/settings/setting-row.slint` | 2 |  |
| `ui/pages/settings/setting-select-row.slint` | `ui/desktop/pages/settings/setting-select-row.slint` | 2 |  |
| `ui/pages/settings/setting-toggle-row.slint` | `ui/desktop/pages/settings/setting-toggle-row.slint` | 1 |  |
| `ui/pages/settings/settings.slint` | `ui/desktop/pages/settings/settings.slint` | 2 | **有未提交改动** |
| `ui/pages/settings/shortcut-popup.slint` | `ui/desktop/pages/settings/shortcut-popup.slint` | 2 |  |
| `ui/pages/settings/sources-popup.slint` | `ui/desktop/pages/settings/sources-popup.slint` | 2 |  |
| `ui/pages/settings/system_tab.slint` | `ui/desktop/pages/settings/system_tab.slint` | 7 |  |
| `ui/pages/toplist/toplist.slint` | `ui/desktop/pages/toplist/toplist.slint` | 3 |  |
| `ui/theme.slint` | `ui/shared/theme.slint` | 0 |  |

## 逐文件引用改动明细

### `ui/appwindow-web.slint` → `ui/desktop/appwindow.web.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 9 | import | `../src/fonts/simhei.ttf` | `../../src/fonts/simhei.ttf` |

### `ui/appwindow.slint` → `ui/desktop/appwindow.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 21 | import | `theme.slint` | `../shared/theme.slint` |
| 22 | import | `language.slint` | `../shared/language.slint` |
| 24 | import | `language.slint` | `../shared/language.slint` |
| 33 | import | `components/models.slint` | `../shared/models.slint` |

### `ui/components/cards.slint` → `ui/desktop/components/cards.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `models.slint` | `../../shared/models.slint` |
| 6 | import | `../theme.slint` | `../../shared/theme.slint` |
| 7 | import | `../language.slint` | `../../shared/language.slint` |
| 33 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 45 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 60 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 99 | asset | `../assets/icons/arrow-more-faint.svg` | `../../assets/icons/arrow-more-faint.svg` |
| 186 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 203 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |

### `ui/components/close-dialog.slint` → `ui/desktop/components/close-dialog.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `../theme.slint` | `../../shared/theme.slint` |
| 7 | import | `../language.slint` | `../../shared/language.slint` |

### `ui/components/desktop-lyrics.slint` → `ui/desktop/components/desktop-lyrics.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 355 | asset | `../assets/icons/unlock.svg` | `../../assets/icons/unlock.svg` |
| 356 | asset | `../assets/icons/lock.svg` | `../../assets/icons/lock.svg` |

### `ui/components/detail.slint` → `ui/desktop/components/detail.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `models.slint` | `../../shared/models.slint` |
| 7 | import | `../language.slint` | `../../shared/language.slint` |
| 77 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 98 | asset | `../assets/icons/heart-gray.svg` | `../../assets/icons/heart-gray.svg` |

### `ui/components/loading.slint` → `ui/desktop/components/loading.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../theme.slint` | `../../shared/theme.slint` |
| 6 | import | `../language.slint` | `../../shared/language.slint` |

### `ui/components/lyrics.slint` → `ui/desktop/components/lyrics.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 7 | import | `models.slint` | `../../shared/models.slint` |
| 10 | import | `../theme.slint` | `../../shared/theme.slint` |
| 11 | import | `../language.slint` | `../../shared/language.slint` |
| 200 | asset | `../assets/icons/playlist-gray.svg` | `../../assets/icons/playlist-gray.svg` |
| 301 | asset | `../assets/icons/clef-staff.svg` | `../../assets/icons/clef-staff.svg` |
| 328 | asset | `../assets/icons/mode-updown-gray.svg` | `../../assets/icons/mode-updown-gray.svg` |
| 329 | asset | `../assets/icons/mode-leftright-gray.svg` | `../../assets/icons/mode-leftright-gray.svg` |
| 353 | asset | `../assets/icons/pip-on-gray.svg` | `../../assets/icons/pip-on-gray.svg` |

### `ui/components/marquee.slint` → `ui/desktop/components/marquee.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 8 | import | `../theme.slint` | `../../shared/theme.slint` |

### `ui/components/media.slint` → `ui/desktop/components/media.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 4 | import | `../theme.slint` | `../../shared/theme.slint` |
| 33 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 59 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 141 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 150 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |

### `ui/components/mini-player.slint` → `ui/desktop/components/mini-player.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 10 | import | `models.slint` | `../../shared/models.slint` |
| 12 | import | `../theme.slint` | `../../shared/theme.slint` |
| 13 | import | `../language.slint` | `../../shared/language.slint` |
| 91 | asset | `../assets/icons/trash-gray.svg` | `../../assets/icons/trash-gray.svg` |
| 121 | asset | `../assets/icons/heart-fill-red.svg` | `../../assets/icons/heart-fill-red.svg` |
| 122 | asset | `../assets/icons/heart-gray.svg` | `../../assets/icons/heart-gray.svg` |
| 234 | asset | `../assets/icons/loader-4.svg` | `../../assets/icons/loader-4.svg` |
| 285 | asset | `../assets/icons/prev-gray.svg` | `../../assets/icons/prev-gray.svg` |
| 306 | asset | `../assets/icons/pause-white.svg` | `../../assets/icons/pause-white.svg` |
| 307 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 327 | asset | `../assets/icons/next-gray.svg` | `../../assets/icons/next-gray.svg` |
| 384 | asset | `../assets/icons/heart-fill-red.svg` | `../../assets/icons/heart-fill-red.svg` |
| 385 | asset | `../assets/icons/heart-gray.svg` | `../../assets/icons/heart-gray.svg` |
| 406 | asset | `../assets/icons/repeat-gray.svg` | `../../assets/icons/repeat-gray.svg` |
| 408 | asset | `../assets/icons/repeat-once-gray.svg` | `../../assets/icons/repeat-once-gray.svg` |
| 409 | asset | `../assets/icons/shuffle-gray.svg` | `../../assets/icons/shuffle-gray.svg` |
| 429 | asset | `../assets/icons/playlist-gray.svg` | `../../assets/icons/playlist-gray.svg` |
| 449 | asset | `../assets/icons/win-close-gray.svg` | `../../assets/icons/win-close-gray.svg` |

### `ui/components/nav.slint` → `ui/desktop/components/nav.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 8 | import | `../theme.slint` | `../../shared/theme.slint` |
| 9 | import | `../language.slint` | `../../shared/language.slint` |
| 130 | asset | `../assets/icons/home-gray.svg` | `../../assets/icons/home-gray.svg` |
| 131 | asset | `../assets/icons/search-gray.svg` | `../../assets/icons/search-gray.svg` |
| 132 | asset | `../assets/icons/playlist-gray.svg` | `../../assets/icons/playlist-gray.svg` |
| 133 | asset | `../assets/icons/album-gray.svg` | `../../assets/icons/album-gray.svg` |
| 134 | asset | `../assets/icons/mv-gray.svg` | `../../assets/icons/mv-gray.svg` |
| 135 | asset | `../assets/icons/radio-gray.svg` | `../../assets/icons/radio-gray.svg` |
| 136 | asset | `../assets/icons/toplist-gray.svg` | `../../assets/icons/toplist-gray.svg` |
| 137 | asset | `../assets/icons/heart-gray.svg` | `../../assets/icons/heart-gray.svg` |
| 138 | asset | `../assets/icons/recent-gray.svg` | `../../assets/icons/recent-gray.svg` |
| 139 | asset | `../assets/icons/local-gray.svg` | `../../assets/icons/local-gray.svg` |
| 145 | asset | `../assets/icons/settings-gray.svg` | `../../assets/icons/settings-gray.svg` |

### `ui/components/playbar.slint` → `ui/desktop/components/playbar.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 10 | import | `models.slint` | `../../shared/models.slint` |
| 11 | import | `../theme.slint` | `../../shared/theme.slint` |
| 146 | asset | `../assets/icons/pip-on-gray.svg` | `../../assets/icons/pip-on-gray.svg` |
| 147 | asset | `../assets/icons/pip-off-gray.svg` | `../../assets/icons/pip-off-gray.svg` |
| 160 | asset | `../assets/icons/loader-4.svg` | `../../assets/icons/loader-4.svg` |
| 211 | asset | `../assets/icons/pause-white.svg` | `../../assets/icons/pause-white.svg` |
| 212 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 253 | asset | `../assets/icons/prev-gray.svg` | `../../assets/icons/prev-gray.svg` |
| 273 | asset | `../assets/icons/next-gray.svg` | `../../assets/icons/next-gray.svg` |
| 304 | asset | `../assets/icons/clef-staff.svg` | `../../assets/icons/clef-staff.svg` |
| 324 | asset | `../assets/icons/volume-mute-line.svg` | `../../assets/icons/volume-mute-line.svg` |
| 326 | asset | `../assets/icons/volume-soft-gray.svg` | `../../assets/icons/volume-soft-gray.svg` |
| 328 | asset | `../assets/icons/volume-mid-gray.svg` | `../../assets/icons/volume-mid-gray.svg` |
| 329 | asset | `../assets/icons/volume-gray.svg` | `../../assets/icons/volume-gray.svg` |
| 354 | asset | `../assets/icons/repeat-gray.svg` | `../../assets/icons/repeat-gray.svg` |
| 356 | asset | `../assets/icons/repeat-once-gray.svg` | `../../assets/icons/repeat-once-gray.svg` |
| 357 | asset | `../assets/icons/shuffle-gray.svg` | `../../assets/icons/shuffle-gray.svg` |
| 375 | asset | `../assets/icons/heart-fill-red.svg` | `../../assets/icons/heart-fill-red.svg` |
| 376 | asset | `../assets/icons/heart-gray.svg` | `../../assets/icons/heart-gray.svg` |
| 393 | asset | `../assets/icons/playlist-gray.svg` | `../../assets/icons/playlist-gray.svg` |

### `ui/components/playlist.slint` → `ui/desktop/components/playlist.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `models.slint` | `../../shared/models.slint` |
| 11 | import | `../theme.slint` | `../../shared/theme.slint` |
| 12 | import | `../language.slint` | `../../shared/language.slint` |
| 86 | asset | `../assets/icons/trash-gray.svg` | `../../assets/icons/trash-gray.svg` |
| 97 | asset | `../assets/icons/win-close-gray.svg` | `../../assets/icons/win-close-gray.svg` |
| 194 | asset | `../assets/icons/heart-fill-red.svg` | `../../assets/icons/heart-fill-red.svg` |
| 195 | asset | `../assets/icons/heart-gray.svg` | `../../assets/icons/heart-gray.svg` |
| 218 | asset | `../assets/icons/trash-gray.svg` | `../../assets/icons/trash-gray.svg` |

### `ui/components/progressbar.slint` → `ui/desktop/components/progressbar.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 4 | import | `../theme.slint` | `../../shared/theme.slint` |

### `ui/components/scroll.slint` → `ui/desktop/components/scroll.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../theme.slint` | `../../shared/theme.slint` |

### `ui/components/scrollable-text-input.slint` → `ui/desktop/components/scrollable-text-input.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 10 | import | `../theme.slint` | `../../shared/theme.slint` |

### `ui/components/song-menu.slint` → `ui/desktop/components/song-menu.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 9 | import | `../theme.slint` | `../../shared/theme.slint` |
| 10 | import | `../language.slint` | `../../shared/language.slint` |
| 227 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 236 | asset | `../assets/icons/playlist-gray.svg` | `../../assets/icons/playlist-gray.svg` |
| 245 | asset | `../assets/icons/heart-fill-red.svg` | `../../assets/icons/heart-fill-red.svg` |
| 246 | asset | `../assets/icons/heart-gray.svg` | `../../assets/icons/heart-gray.svg` |
| 257 | asset | `../assets/icons/trash-gray.svg` | `../../assets/icons/trash-gray.svg` |

### `ui/components/songlist.slint` → `ui/desktop/components/songlist.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `models.slint` | `../../shared/models.slint` |
| 7 | import | `../theme.slint` | `../../shared/theme.slint` |
| 8 | import | `../language.slint` | `../../shared/language.slint` |
| 47 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |

### `ui/components/splash.slint` → `ui/desktop/components/splash.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../theme.slint` | `../../shared/theme.slint` |
| 106 | asset | `../assets/splash.png` | `../../assets/splash.png` |

### `ui/components/suggest.slint` → `ui/desktop/components/suggest.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 7 | import | `../theme.slint` | `../../shared/theme.slint` |
| 8 | import | `../language.slint` | `../../shared/language.slint` |
| 9 | import | `models.slint` | `../../shared/models.slint` |
| 56 | asset | `../assets/icons/search-faint.svg` | `../../assets/icons/search-faint.svg` |

### `ui/components/tab-bar.slint` → `ui/desktop/components/tab-bar.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 9 | import | `../theme.slint` | `../../shared/theme.slint` |

### `ui/components/titlebar.slint` → `ui/desktop/components/titlebar.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `../theme.slint` | `../../shared/theme.slint` |
| 7 | import | `../language.slint` | `../../shared/language.slint` |
| 8 | import | `./scrollable-text-input.slint` | `scrollable-text-input.slint` |
| 111 | asset | `../assets/logo.png` | `../../assets/logo.png` |
| 150 | asset | `../assets/icons/search-faint.svg` | `../../assets/icons/search-faint.svg` |
| 222 | asset | `../assets/icons/caret-down-gray.svg` | `../../assets/icons/caret-down-gray.svg` |
| 241 | asset | `../assets/icons/settings-gray.svg` | `../../assets/icons/settings-gray.svg` |
| 260 | asset | `../assets/icons/moon.svg` | `../../assets/icons/moon.svg` |
| 261 | asset | `../assets/icons/sun.svg` | `../../assets/icons/sun.svg` |
| 280 | asset | `../assets/icons/win-min-gray.svg` | `../../assets/icons/win-min-gray.svg` |
| 299 | asset | `../assets/icons/win-minimize-gray.svg` | `../../assets/icons/win-minimize-gray.svg` |
| 319 | asset | `../assets/icons/win-close-white.svg` | `../../assets/icons/win-close-white.svg` |
| 320 | asset | `../assets/icons/win-close-gray.svg` | `../../assets/icons/win-close-gray.svg` |

### `ui/components/toast.slint` → `ui/desktop/components/toast.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 4 | import | `../theme.slint` | `../../shared/theme.slint` |

### `ui/components/tray-menu.slint` → `ui/desktop/components/tray-menu.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 9 | import | `../theme.slint` | `../../shared/theme.slint` |
| 10 | import | `../language.slint` | `../../shared/language.slint` |
| 12 | import | `../language.slint` | `../../shared/language.slint` |

### `ui/components/update-dialog.slint` → `ui/desktop/components/update-dialog.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 9 | import | `../theme.slint` | `../../shared/theme.slint` |
| 10 | import | `../language.slint` | `../../shared/language.slint` |

### `ui/mobile/appwindow-mobile.slint` → `ui/mobile/appwindow-mobile.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 7 | import | `../theme.slint` | `../shared/theme.slint` |
| 16 | import | `../components/models.slint` | `../shared/models.slint` |

### `ui/mobile/components/glass.slint` → `ui/mobile/components/glass.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `../../theme.slint` | `../../shared/theme.slint` |

### `ui/mobile/components/mini-player.slint` → `ui/mobile/components/mini-player.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../../theme.slint` | `../../shared/theme.slint` |

### `ui/mobile/components/nav.slint` → `ui/mobile/components/nav.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 8 | import | `../../theme.slint` | `../../shared/theme.slint` |

### `ui/mobile/components/playbar.slint` → `ui/mobile/components/playbar.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../../theme.slint` | `../../shared/theme.slint` |

### `ui/mobile/components/song-row.slint` → `ui/mobile/components/song-row.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../../theme.slint` | `../../shared/theme.slint` |

### `ui/mobile/pages/detail.slint` → `ui/mobile/pages/detail.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 8 | import | `../../theme.slint` | `../../shared/theme.slint` |
| 10 | import | `../../components/models.slint` | `../../shared/models.slint` |

### `ui/mobile/pages/home.slint` → `ui/mobile/pages/home.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `../../theme.slint` | `../../shared/theme.slint` |
| 8 | import | `../../components/models.slint` | `../../shared/models.slint` |

### `ui/mobile/pages/player.slint` → `ui/mobile/pages/player.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 9 | import | `../../components/models.slint` | `../../shared/models.slint` |

### `ui/mobile/pages/search.slint` → `ui/mobile/pages/search.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 7 | import | `../../theme.slint` | `../../shared/theme.slint` |
| 9 | import | `../../components/models.slint` | `../../shared/models.slint` |

### `ui/mobile/pages/settings.slint` → `ui/mobile/pages/settings.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 8 | import | `../../theme.slint` | `../../shared/theme.slint` |

### `ui/mobile/theme.slint` → `ui/mobile/theme.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `../theme.slint` | `../shared/theme.slint` |

### `ui/pages/albums/albums.slint` → `ui/desktop/pages/albums/albums.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 8 | import | `../../components/models.slint` | `../../../shared/models.slint` |
| 9 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 10 | import | `../../language.slint` | `../../../shared/language.slint` |

### `ui/pages/detail.slint` → `ui/desktop/pages/detail.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 8 | import | `../components/models.slint` | `../../shared/models.slint` |
| 11 | import | `../theme.slint` | `../../shared/theme.slint` |
| 12 | import | `../language.slint` | `../../shared/language.slint` |
| 65 | asset | `../assets/icons/arrow-back-up.svg` | `../../assets/icons/arrow-back-up.svg` |
| 99 | asset | `../assets/icons/search-faint.svg` | `../../assets/icons/search-faint.svg` |
| 134 | asset | `../assets/icons/layout-rows-gray.svg` | `../../assets/icons/layout-rows-gray.svg` |
| 135 | asset | `../assets/icons/layout-columns-gray.svg` | `../../assets/icons/layout-columns-gray.svg` |
| 152 | asset | `../assets/icons/heart-fill-red.svg` | `../../assets/icons/heart-fill-red.svg` |
| 153 | asset | `../assets/icons/heart-gray.svg` | `../../assets/icons/heart-gray.svg` |
| 172 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 252 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 338 | asset | `../assets/icons/heart-fill-red.svg` | `../../assets/icons/heart-fill-red.svg` |
| 339 | asset | `../assets/icons/heart-gray.svg` | `../../assets/icons/heart-gray.svg` |
| 351 | asset | `../assets/icons/pause-white.svg` | `../../assets/icons/pause-white.svg` |
| 352 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 399 | asset | `../assets/icons/heart-fill-red.svg` | `../../assets/icons/heart-fill-red.svg` |
| 400 | asset | `../assets/icons/heart-gray.svg` | `../../assets/icons/heart-gray.svg` |
| 412 | asset | `../assets/icons/pause-white.svg` | `../../assets/icons/pause-white.svg` |
| 413 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |

### `ui/pages/home/home.slint` → `ui/desktop/pages/home/home.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 7 | import | `../../components/models.slint` | `../../../shared/models.slint` |
| 9 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 10 | import | `../../language.slint` | `../../../shared/language.slint` |
| 81 | asset | `../../assets/icons/arrow-more-faint.svg` | `../../../assets/icons/arrow-more-faint.svg` |
| 222 | asset | `../../assets/icons/play-white.svg` | `../../../assets/icons/play-white.svg` |

### `ui/pages/liked/liked.slint` → `ui/desktop/pages/liked/liked.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `../../components/models.slint` | `../../../shared/models.slint` |
| 9 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 10 | import | `../../language.slint` | `../../../shared/language.slint` |
| 70 | asset | `../../assets/icons/heart-fill-red.svg` | `../../../assets/icons/heart-fill-red.svg` |
| 152 | asset | `../../assets/icons/heart-fill-red.svg` | `../../../assets/icons/heart-fill-red.svg` |

### `ui/pages/local/local.slint` → `ui/desktop/pages/local/local.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `../../components/models.slint` | `../../../shared/models.slint` |
| 9 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 10 | import | `../../language.slint` | `../../../shared/language.slint` |
| 49 | asset | `../../assets/icons/mv-gray.svg` | `../../../assets/icons/mv-gray.svg` |
| 160 | asset | `../../assets/icons/folder.svg` | `../../../assets/icons/folder.svg` |
| 229 | asset | `../../assets/icons/search-gray.svg` | `../../../assets/icons/search-gray.svg` |

### `ui/pages/mv/mv.slint` → `ui/desktop/pages/mv/mv.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 8 | import | `../../components/models.slint` | `../../../shared/models.slint` |
| 9 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 10 | import | `../../language.slint` | `../../../shared/language.slint` |

### `ui/pages/mv_player.slint` → `ui/desktop/pages/mv_player.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `../theme.slint` | `../../shared/theme.slint` |
| 7 | import | `../language.slint` | `../../shared/language.slint` |
| 8 | import | `../components/models.slint` | `../../shared/models.slint` |
| 88 | asset | `../assets/tabler/icons/filled/player-play.svg` | `../../assets/tabler/icons/filled/player-play.svg` |
| 109 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 711 | asset | `../assets/tabler/icons/outline/arrow-left-to-arc.svg` | `../../assets/tabler/icons/outline/arrow-left-to-arc.svg` |
| 740 | asset | `../assets/icons/pip-on-gray.svg` | `../../assets/icons/pip-on-gray.svg` |
| 751 | asset | `../assets/tabler/icons/outline/arrows-minimize.svg` | `../../assets/tabler/icons/outline/arrows-minimize.svg` |
| 752 | asset | `../assets/tabler/icons/outline/arrows-maximize.svg` | `../../assets/tabler/icons/outline/arrows-maximize.svg` |
| 767 | asset | `../assets/icons/pip-off-gray.svg` | `../../assets/icons/pip-off-gray.svg` |
| 800 | asset | `../assets/icons/prev-gray.svg` | `../../assets/icons/prev-gray.svg` |
| 811 | asset | `../assets/icons/pause-white.svg` | `../../assets/icons/pause-white.svg` |
| 812 | asset | `../assets/icons/play-white.svg` | `../../assets/icons/play-white.svg` |
| 824 | asset | `../assets/icons/next-gray.svg` | `../../assets/icons/next-gray.svg` |
| 902 | asset | `../assets/tabler/icons/outline/badge-sd.svg` | `../../assets/tabler/icons/outline/badge-sd.svg` |
| 904 | asset | `../assets/tabler/icons/outline/badge-hd.svg` | `../../assets/tabler/icons/outline/badge-hd.svg` |
| 905 | asset | `../assets/tabler/icons/outline/badge-2k.svg` | `../../assets/tabler/icons/outline/badge-2k.svg` |
| 923 | asset | `../assets/icons/volume-mute-line.svg` | `../../assets/icons/volume-mute-line.svg` |
| 925 | asset | `../assets/icons/volume-soft-gray.svg` | `../../assets/icons/volume-soft-gray.svg` |
| 927 | asset | `../assets/icons/volume-mid-gray.svg` | `../../assets/icons/volume-mid-gray.svg` |
| 928 | asset | `../assets/icons/volume-gray.svg` | `../../assets/icons/volume-gray.svg` |
| 980 | asset | `../assets/tabler/icons/outline/playlist.svg` | `../../assets/tabler/icons/outline/playlist.svg` |
| 1042 | asset | `../assets/tabler/icons/outline/badge-sd.svg` | `../../assets/tabler/icons/outline/badge-sd.svg` |
| 1044 | asset | `../assets/tabler/icons/outline/badge-hd.svg` | `../../assets/tabler/icons/outline/badge-hd.svg` |
| 1045 | asset | `../assets/tabler/icons/outline/badge-2k.svg` | `../../assets/tabler/icons/outline/badge-2k.svg` |
| 1060 | asset | `../assets/tabler/icons/outline/check.svg` | `../../assets/tabler/icons/outline/check.svg` |
| 1121 | asset | `../assets/icons/win-close-gray.svg` | `../../assets/icons/win-close-gray.svg` |
| 1148 | asset | `../assets/icons/search-faint.svg` | `../../assets/icons/search-faint.svg` |
| 1191 | asset | `../assets/icons/win-close-gray.svg` | `../../assets/icons/win-close-gray.svg` |
| 1323 | asset | `../assets/icons/search-faint.svg` | `../../assets/icons/search-faint.svg` |

### `ui/pages/placeholder/placeholder.slint` → `ui/desktop/pages/placeholder/placeholder.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 4 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 5 | import | `../../language.slint` | `../../../shared/language.slint` |

### `ui/pages/playlists/playlists.slint` → `ui/desktop/pages/playlists/playlists.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 10 | import | `../../components/models.slint` | `../../../shared/models.slint` |
| 11 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 12 | import | `../../language.slint` | `../../../shared/language.slint` |

### `ui/pages/radio/radio.slint` → `ui/desktop/pages/radio/radio.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 8 | import | `../../components/models.slint` | `../../../shared/models.slint` |
| 9 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 10 | import | `../../language.slint` | `../../../shared/language.slint` |

### `ui/pages/recent/recent.slint` → `ui/desktop/pages/recent/recent.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../../components/models.slint` | `../../../shared/models.slint` |
| 8 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 9 | import | `../../language.slint` | `../../../shared/language.slint` |
| 56 | asset | `../../assets/icons/trash-gray.svg` | `../../../assets/icons/trash-gray.svg` |
| 136 | asset | `../../assets/icons/trash-gray.svg` | `../../../assets/icons/trash-gray.svg` |
| 211 | asset | `../../assets/icons/recent-gray.svg` | `../../../assets/icons/recent-gray.svg` |

### `ui/pages/search/search.slint` → `ui/desktop/pages/search/search.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 7 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 8 | import | `../../language.slint` | `../../../shared/language.slint` |
| 9 | import | `../../components/models.slint` | `../../../shared/models.slint` |
| 311 | asset | `../../assets/icons/arrow-back-up.svg` | `../../../assets/icons/arrow-back-up.svg` |
| 454 | asset | `../../assets/icons/play-white.svg` | `../../../assets/icons/play-white.svg` |

### `ui/pages/settings/about_tab.slint` → `ui/desktop/pages/settings/about_tab.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 7 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 8 | import | `../../language.slint` | `../../../shared/language.slint` |
| 30 | asset | `../../assets/icons/versions.svg` | `../../../assets/icons/versions.svg` |
| 40 | asset | `../../assets/icons/pencil-code.svg` | `../../../assets/icons/pencil-code.svg` |

### `ui/pages/settings/app_tab.slint` → `ui/desktop/pages/settings/app_tab.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 10 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 11 | import | `../../language.slint` | `../../../shared/language.slint` |
| 109 | asset | `../../assets/icons/shirt.svg` | `../../../assets/icons/shirt.svg` |
| 121 | asset | `../../assets/icons/layout-rows-gray.svg` | `../../../assets/icons/layout-rows-gray.svg` |
| 133 | asset | `../../assets/icons/clef-staff.svg` | `../../../assets/icons/clef-staff.svg` |
| 142 | asset | `../../assets/icons/pip-on-gray.svg` | `../../../assets/icons/pip-on-gray.svg` |
| 167 | asset | `../../assets/icons/clef-staff.svg` | `../../../assets/icons/clef-staff.svg` |
| 175 | asset | `../../assets/icons/win-close-gray.svg` | `../../../assets/icons/win-close-gray.svg` |
| 187 | asset | `../../assets/icons/keyboard.svg` | `../../../assets/icons/keyboard.svg` |

### `ui/pages/settings/base_tab.slint` → `ui/desktop/pages/settings/base_tab.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 8 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 9 | import | `../../language.slint` | `../../../shared/language.slint` |
| 36 | asset | `../../assets/icons/sun.svg` | `../../../assets/icons/sun.svg` |
| 47 | asset | `../../assets/icons/power.svg` | `../../../assets/icons/power.svg` |
| 56 | asset | `../../assets/icons/language.svg` | `../../../assets/icons/language.svg` |
| 68 | asset | `../../assets/icons/typography.svg` | `../../../assets/icons/typography.svg` |

### `ui/pages/settings/donate_tab.slint` → `ui/desktop/pages/settings/donate_tab.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 6 | import | `../../language.slint` | `../../../shared/language.slint` |
| 37 | asset | `../../assets/donate/donate-qr.png` | `../../../assets/donate/donate-qr.png` |

### `ui/pages/settings/network_tab.slint` → `ui/desktop/pages/settings/network_tab.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 7 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 8 | import | `../../language.slint` | `../../../shared/language.slint` |
| 41 | asset | `../../assets/icons/arrow-big-down-lines.svg` | `../../../assets/icons/arrow-big-down-lines.svg` |
| 73 | asset | `../../assets/icons/cloud-network.svg` | `../../../assets/icons/cloud-network.svg` |
| 132 | asset | `../../assets/icons/building-broadcast-tower.svg` | `../../../assets/icons/building-broadcast-tower.svg` |

### `ui/pages/settings/play_tab.slint` → `ui/desktop/pages/settings/play_tab.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 9 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 10 | import | `../../language.slint` | `../../../shared/language.slint` |
| 45 | asset | `../../assets/icons/circuit-battery.svg` | `../../../assets/icons/circuit-battery.svg` |
| 57 | asset | `../../assets/icons/clef-staff.svg` | `../../../assets/icons/clef-staff.svg` |
| 65 | asset | `../../assets/icons/circle-play.svg` | `../../../assets/icons/circle-play.svg` |
| 73 | asset | `../../assets/icons/device-speaker.svg` | `../../../assets/icons/device-speaker.svg` |
| 96 | asset | `../../assets/icons/database-import.svg` | `../../../assets/icons/database-import.svg` |
| 103 | asset | `../../assets/icons/refresh-dot.svg` | `../../../assets/icons/refresh-dot.svg` |
| 109 | asset | `../../assets/icons/cloud-network.svg` | `../../../assets/icons/cloud-network.svg` |
| 116 | asset | `../../assets/icons/cloud-network.svg` | `../../../assets/icons/cloud-network.svg` |

### `ui/pages/settings/proxy-popup.slint` → `ui/desktop/pages/settings/proxy-popup.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 6 | import | `../../language.slint` | `../../../shared/language.slint` |

### `ui/pages/settings/select-popup.slint` → `ui/desktop/pages/settings/select-popup.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 56 | asset | `../../assets/icons/check.svg` | `../../../assets/icons/check.svg` |

### `ui/pages/settings/setting-input-row.slint` → `ui/desktop/pages/settings/setting-input-row.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../../theme.slint` | `../../../shared/theme.slint` |

### `ui/pages/settings/setting-row.slint` → `ui/desktop/pages/settings/setting-row.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 37 | asset | `../../assets/icons/arrow-more-faint.svg` | `../../../assets/icons/arrow-more-faint.svg` |

### `ui/pages/settings/setting-select-row.slint` → `ui/desktop/pages/settings/setting-select-row.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 6 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 45 | asset | `../../assets/icons/arrow-more-faint.svg` | `../../../assets/icons/arrow-more-faint.svg` |

### `ui/pages/settings/setting-toggle-row.slint` → `ui/desktop/pages/settings/setting-toggle-row.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../../theme.slint` | `../../../shared/theme.slint` |

### `ui/pages/settings/settings.slint` → `ui/desktop/pages/settings/settings.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 10 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 11 | import | `../../language.slint` | `../../../shared/language.slint` |

### `ui/pages/settings/shortcut-popup.slint` → `ui/desktop/pages/settings/shortcut-popup.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 7 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 8 | import | `../../language.slint` | `../../../shared/language.slint` |

### `ui/pages/settings/sources-popup.slint` → `ui/desktop/pages/settings/sources-popup.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 5 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 6 | import | `../../language.slint` | `../../../shared/language.slint` |

### `ui/pages/settings/system_tab.slint` → `ui/desktop/pages/settings/system_tab.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 12 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 13 | import | `../../language.slint` | `../../../shared/language.slint` |
| 52 | asset | `../../assets/icons/database-cog.svg` | `../../../assets/icons/database-cog.svg` |
| 60 | asset | `../../assets/icons/folder.svg` | `../../../assets/icons/folder.svg` |
| 66 | asset | `../../assets/icons/database-import.svg` | `../../../assets/icons/database-import.svg` |
| 74 | asset | `../../assets/icons/database-x.svg` | `../../../assets/icons/database-x.svg` |
| 123 | asset | `../../assets/icons/refresh-dot.svg` | `../../../assets/icons/refresh-dot.svg` |

### `ui/pages/toplist/toplist.slint` → `ui/desktop/pages/toplist/toplist.slint`

| 行 | 类型 | 旧值 | 新值 |
|---|---|---|---|
| 7 | import | `../../components/models.slint` | `../../../shared/models.slint` |
| 8 | import | `../../theme.slint` | `../../../shared/theme.slint` |
| 9 | import | `../../language.slint` | `../../../shared/language.slint` |

## 非 .slint 侧需要同步的位置

| 位置 | 现状 | 需改为 |
|---|---|---|
| `build.rs` wasm 分支 | `ui/appwindow-web.slint` | `ui/desktop/appwindow.web.slint` |
| `build.rs` 桌面/默认 | `ui/appwindow.slint` | `ui/desktop/appwindow.slint` |
| `build.rs` 移动端 | `ui/mobile/appwindow-mobile.slint` | 不变（可选重命名为 `ui/mobile/appwindow.slint`） |
| `build.rs` 图标 | `ui/assets/app.ico` | 不变 |
| `src/i18n/mod.rs` | `include_str!("../../ui/assets/lang/*.json")` | 不变 |
| `src/platform/tray.rs` / `src/main.rs` | `include_bytes!("../../ui/assets/app-icon.png")` | 不变 |
| 文档注释 | `src/mobile/entry.rs:86`、`src/platform/tray.rs:2`、`src/persist/config.rs:329` 提到旧路径 | 建议同步更新注释 |

## 风险与执行顺序

1. **资产路径**：`ui/assets` 不动，故 desktop/components 与 desktop/pages 的 `@image-url` 全部需要多一层 `../`（清单已逐行列出）。
2. **未提交改动**：表中标记「有未提交改动」的文件在迁移时只提交“重命名 + 引用改写”，其业务改动留在工作区（用 index 精确构造，不卷入提交）。
3. **共享层**：`models.slint` 移入 `shared/` 后，桌面侧引用会从 `models.slint` 变成 `../../shared/models.slint` 等，清单已包含。
4. **web wrapper**：保留，仅移动到 `ui/desktop/appwindow.web.slint`，字体路径改为 `../../src/fonts/simhei.ttf`。
5. **验证**：先 `cargo check`（桌面）与 `cargo check --no-default-features --features mobile`，再本地 `cargo run` 目视桌面窗口；最后触发 Mobile CI。
