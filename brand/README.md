# Name and logo

People see the same name and logo everywhere TwoViewer appears (UI-01). That covers the web client, the host page, the splash screen, emails, installers, the session indicator and the Host Agent's name in the operating system.

## Name

TwoViewer: one word, with a capital T and V. It is never translated, shortened or split ("Twoviewer", "Two Viewer", "TV").

- The web client takes it from `PRODUCT_NAME` in [`web/src/app/product.ts`](../web/src/app/product.ts). `npm run check:i18n` fails on text that spells it out instead.
- The Host Agent appears in the operating system as TwoViewer, for its app, its service and its process description. QuickShare appears as TwoViewer QuickShare.
- Lowercase `twoviewer` is only for names people don't see, such as crates, environment variables, the database and container images.

## Logo

[`logo.svg`](logo.svg) is the logo, and the only file to edit. It shows two screens, the front one solid, in white on a rounded blue tile (`#1F5FD6`).

- Use it as it is: don't redraw, recolor, stretch or crop it, and don't add effects.
- The tile is blue in both themes. White on it has a contrast of 5.7:1, and the tile has 3.2:1 against the dark background and 5.7:1 against white.
- Show it at 16 px or larger, with the name to its right where there is room.
- It is drawn on a 48-unit grid, with its edges on multiples of 3, so that it stays sharp at 16 and 32 px. Keep to that grid when you change it, and look at the 16 px favicon afterwards.

## Where it is used

| Place | File | Issue |
|---|---|---|
| The web client's header, including the host page, and the browser tab | `web/public/logo.svg` | #16 |
| The browser tab, in browsers without SVG icons | `web/public/favicon.ico` (16, 32 and 48 px) | #16 |
| Bookmarks and home-screen shortcuts on iOS | `web/public/apple-touch-icon.png` (180 px) | #16 |
| The splash screen | | #21 |
| Emails, as PNG because email clients don't show SVG (96 px, shown at 48 px) | `server/templates/email/logo.png` | #22 |
| Installers, and the Host Agent's icon in the operating system | | #34 |
| The session indicator | | #39 |

`cargo xtask icons` writes these files from `logo.svg`. `cargo xtask check-icons`, which CI runs, fails if one is missing or doesn't match the logo, so don't edit them by hand. To use the logo in a new place, add its file to `ICONS` in [`xtask/src/icons.rs`](../xtask/src/icons.rs) instead of exporting the logo yourself.
