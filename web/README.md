# TwoViewer web client

This project was generated using [Angular CLI](https://github.com/angular/angular-cli) version 22.2.4.

## Development server

To start a local development server, run:

```bash
ng serve
```

Once the server is running, open your browser and navigate to `http://localhost:4200/`. The application will automatically reload whenever you modify any of the source files.

## Code scaffolding

Angular CLI includes powerful code scaffolding tools. To generate a new component, run:

```bash
ng generate component component-name
```

For a complete list of available schematics (such as `components`, `directives`, or `pipes`), run:

```bash
ng generate --help
```

## Building

To build the project run:

```bash
ng build
```

This will compile your project and store the build artifacts in the `dist/` directory. By default, the production build optimizes your application for performance and speed.

## Running unit tests

To execute unit tests with the [Vitest](https://vitest.dev/) test runner, use the following command:

```bash
ng test
```

## Screens and layouts

[`src/app/app.routes.ts`](src/app/app.routes.ts) has a route for every screen in §4.1 of the SRS. Each screen is in the folder of its feature (`accounts/`, `devices/`, `sessions/`, `vpn/`, `audit/`, `settings/`, `operations/`, `legal/`, `errors/`), and its code is downloaded when it is first opened. A screen sits in one of two layouts:

- `layout/app-layout`: the signed-in screens, with the main navigation and the *Account* menu (`layout/account-menu`), which has the theme and a link to Settings. Below 48rem wide, the navigation opens from a *Menu* button.
- `layout/plain-layout`: the visitor screens, the host page, the policies and the error pages, with the logo and product name only.

The remote session view has no layout and fills the window. The root component, [`src/app/app.html`](src/app/app.html), adds what every page has: a skip link, the footer with the Terms of Service and Privacy Notice (PRIV-04), the toasts and the confirmation dialog. `/terms` and `/privacy` are placeholders until the texts are written.

Unknown addresses show *Page not found*. When a screen can't be loaded, for example because its code can't be downloaded, `errors/navigation-error.ts` shows *You're offline* or *This page couldn't be loaded* and keeps the address in the address bar. *Retry* loads that address as a new page, because browsers remember a module that failed to download, and only a new page downloads it again. The offline page has no *Retry*: it loads the page on its own once the browser is back online. Both pages are bundled with the app rather than downloaded later, so they can still be shown when downloads fail.

## Shared components

In [`src/app/shared/`](src/app/shared):

| Use | For |
|---|---|
| `Confirmation.confirm({ title, message, confirmLabel, typeToConfirm? })` | Destructive actions (UI-03). It resolves to `true` if the user confirms. With `typeToConfirm`, such as the account's email address, the user must type it first. |
| `<app-empty-state>` | A list with nothing in it. Say what the next step is, and link to it with an element marked `actions` (UI-04). |
| `describeError(error)`, `<app-error-message [error]>` | Errors: what happened and what the user can do, never server messages or status codes (UI-06). Describe the errors a feature expects yourself, as a `UserError`, which both take as it is, and use these for the rest. |
| `Toasts.show(message)` | Confirming that a background action has finished (UI-08). A toast disappears after 5 seconds and pauses while hovered or focused. The 5 seconds is a constant, not a server setting (MNT-01), until the web client gets settings from the server. |
| `localeDate`, `localeNumber` pipes | Dates, times and numbers ([below](#dates-times-and-numbers)). |
| `ServerConnection` | The real-time connection to the server (IF-09, [below](#real-time-updates)). |
| `Theme`, `<app-theme-picker>` | The theme option, and the radio buttons that choose it ([below](#theme)). |
| `Motion`, `<app-motion-picker>` | The motion option, and the radio buttons that choose it ([below](#motion)). |
| `REDUCED_MOTION` | Whether motion should be reduced right now (ANI-04), from the motion option and the system's preference. Code that starts an animation itself, such as a view transition, checks it first. Styles don't need it ([below](#reduced-motion)). |
| `<app-skeleton [rows]>` | A list that is loading, such as the devices or the audit log: placeholder rows that shimmer (ANI-08). |

Style with the color tokens in [`src/styles.scss`](src/styles.scss) (`var(--color-text)` and so on), never fixed colors, so that every component works in both themes (THM-06) with enough contrast ([below](#colors-and-contrast)). CI fails on hex colors and color functions in component styles. Animate with the motion tokens, never fixed durations or distances, so that reduced motion applies ([below](#motion)).

## Real-time updates

`ServerConnection` (in `src/app/shared/connection/`) keeps a WebSocket open to the server, so that screens update without a reload (UI-02). `AppLayout` starts it, so it is open on every signed-in screen. A screen that shows server state:

- fetches it through the API, and again on every `resyncs`, which fires after each connection and whenever events may have been missed;
- subscribes to `events` and fetches what an event says has changed. Events name what changed, never the new state.

The messages are described in [`protocol/src/lib.rs`](../protocol/src/lib.rs), and copied in `messages.ts`.

After every interruption, it reconnects with randomized, increasing delays (REL-05): between half and all of 1, 2, 4, … seconds, at most 30. It reconnects at once when the browser is back online or the tab is shown again. When the connection has been down for 2 seconds, `<app-connection-banner>` at the top of every page says so, without blocking anything (UI-05); a restart of the server reconnects faster and doesn't show it. These numbers are constants, like the toast duration, until the web client gets settings from the server.

In specs, `provideFakeSockets()` from `shared/connection/testing.ts` replaces the WebSocket with fakes on which the spec plays the server. Specs that render `AppLayout` need it.

### With the development server

`npm start` proxies `/api`, including the WebSocket, to the server on `https://localhost:8443` ([`proxy.conf.json`](proxy.conf.json)), so start the server side first ([`../README.md`](../README.md#running-the-server-side)). The proxy accepts the self-signed certificate, and rewrites the `Origin` of WebSocket handshakes to the server's own, which the server requires. Without the server, the banner shows that the connection is lost.

## The API

Screens call the server's JSON API under `/api` with Angular's `HttpClient` (`provideHttpClient` in `app.config.ts`). Bodies have camelCase keys. An error has a JSON body with a snake_case `error` code, and a `422` lists the fields that break the rules, with why ([`server/src/api.rs`](../server/src/api.rs)). A screen handles the answers it expects, such as showing a `422` next to its fields, and passes the rest to `<app-error-message>`. In specs, `provideHttpClientTesting()` lets the spec answer the requests.

## Accounts

The screens are in `src/app/accounts/`.

- **Sign up** (AUTH-01) checks the fields before sending them, with the server's default rules in `account-rules.ts`. They are constants until the web client gets settings from the server, and the server's answers say which limit a field didn't meet. The page sends the versions of the Terms of Service and Privacy Notice in [`src/app/legal/versions.ts`](src/app/legal/versions.ts): change a version whenever its text changes, together with the server's. The server answers the same whether or not the address is registered (SEC-14), so the page always says to check the email.
- **Proof of work** (AUTH-15). When a network has signed up often, the server answers `428` with a challenge. `ProofOfWork` solves it in a web worker, `proof-of-work.worker.ts`, so that the page stays responsive, and the page sends the sign-up again with the solution. The worker imports only `proof-of-work-solver.ts`, without Angular, to stay small, and is type-checked with `tsconfig.worker.json`. Specs replace `ProofOfWork`, since jsdom has no workers.
- **Verify your email address** (AUTH-02) is where the link in the email leads: `/verify-email#<token>`. The token is in the fragment, which browsers never send to a server. The page takes it out of the address bar, then sends it to the server.

## Name and logo

Both layouts start with the logo and the product name, which link to the home screen (UI-01). The remote session view has no header, and shows them in the browser tab. The logo, [`public/logo.svg`](public/logo.svg), and the icons beside it (`favicon.ico` and `apple-touch-icon.png`) are made from [`brand/logo.svg`](../brand/logo.svg) by `cargo xtask icons`, so don't edit them here. [`brand/README.md`](../brand/README.md) has the rules for the name and the logo.

## Theme

The theme option is Light, Dark or System, and System is the default (THM-01). Every color token is a `light-dark()` pair, except the hint overlay's ([below](#colors-and-contrast)), so the page follows `color-scheme` on `<html>`:

- With System, `color-scheme` is `light dark`, and the browser switches the colors, native controls and scrollbars whenever the system's preference changes, without any code (THM-02).
- Light and Dark set `data-theme="light"` or `"dark"` on `<html>`, which fixes `color-scheme`.

`Theme` (in `src/app/shared/theme/`) holds the option. `Theme.choose()` applies it at once with a cross-fade of 150 ms, a view transition that doesn't block clicks, or without one when motion is reduced (THM-03). It remembers the option in this browser's `localStorage`, so that the pages before sign-in use it (THM-04), and other open tabs follow. `<app-theme-picker>` is the choice as radio buttons, in Settings → Appearance and in the *Account* menu (THM-08).

[`public/appearance.js`](public/appearance.js) applies the remembered option before the first paint, so that no frame, including the splash screen, shows the other theme (THM-05). It applies the remembered motion option too ([below](#reduced-motion)). `index.html` loads it in `<head>` without `async` or `defer`, so that it runs before the page is drawn. Keep it a small file: the Content Security Policy blocks inline scripts. It reads the same `localStorage` keys as `Theme` and `Motion`, and specs run it to check that. Its name has no content hash, so once the server serves the web client, it must be served so that browsers check for a newer version.

Saving the option in the user's preferences, and applying it in every browser after sign-in (THM-04), comes with sign-in. The comment on `Theme.choose()` says where.

### Colors and contrast

Both palettes meet WCAG 2.1 AA contrast (THM-07): 4.5:1 for text, and 3:1 for the borders of controls, focus indicators and status graphics. The color tokens, without `--color-`, are for:

| Tokens | For |
|---|---|
| `bg`, `surface` | Backgrounds: the page, dialogs and menus; the header, buttons, toasts and banners. |
| `text`, `text-muted` | Text, and secondary text such as the footer and placeholders. |
| `border-strong` | The borders of controls, such as inputs and buttons. `border` is for separators, which need no contrast. |
| `accent`, `on-accent`, `focus` | Links, the current page, primary buttons and the focus indicator. |
| `danger`, `on-danger` | Errors and destructive actions (UI-03). |
| `success`, `on-success`, `warning`, `on-warning` | Statuses, such as a device being online, or the warning before an idle session ends (SES-10). |
| `hint`, `on-hint` | The hint overlay (VIM-14). |

[`src/app/shared/theme/palette.spec.ts`](src/app/shared/theme/palette.spec.ts) checks every pair of tokens that is used together, in both themes. Use a color only on a background it is paired with there, and an `on-` color only on its own color; for a new combination, add the pair to the spec. The spec also fails on a color token that is in no pair, unless it lists the token as decorative and says why. Controls that can't be used are exempt, which is why disabled buttons can be faded.

In a remote session, the toolbar and the status bar have opaque `surface` backgrounds, never translucent ones over the remote screen, so that their contrast doesn't depend on what the remote screen shows. The hint overlay lies on the remote screen itself, whose colors the theme never changes (THM-09), so its colors are the same in both themes. Draw every label and grid line in both of them, such as a `hint` label with `on-hint` text and outline: they differ by more than 9:1, so one of them has at least 3:1 against any color of the remote screen.

## Motion

Animation shows what has changed (§4.4). These animate (ANI-01):

| What | How |
|---|---|
| Moving to another screen | The page cross-fades: the router's view transition, timed by `screenTransition` (in `src/app/shared/motion/`). Not on the first screen of a page load. |
| Dialogs | They grow and fade in, and fade out. |
| Menus and panels, such as the *Account* menu and the navigation in narrow windows | They drop in and fade, and fade out. |
| Toasts and the connection banner | They slide in and fade, and fade out, with Angular's `animate.enter` and `animate.leave`. |
| Progress without a known end, such as reconnecting | `<span class="spinner" aria-hidden="true"></span>`, next to text that says what is happening. |
| Lists that are loading | `<app-skeleton [rows]>`, with about as many rows as the list usually has (ANI-08). |
| The theme | A cross-fade ([above](#theme)). |

The splash screen, the session toolbar, Vim mode and device status come with their features, and follow the same rules:

- Take durations, easings, distances and endless animations from the motion tokens in [`src/styles.scss`](src/styles.scss). Both durations are within 100–300 ms (ANI-02); only the splash screen's logo animation may last up to 1 second.

  | Tokens | For |
  |---|---|
  | `--duration-medium`, `--ease-out` | What appears. |
  | `--duration-short`, `--ease-in` | What goes away. |
  | `--ease-in-out` | What changes in place, such as a cross-fade. |
  | `--motion-distance`, `--motion-scale` | How far things slide, and the scale that dialogs grow from. |
  | `--animation-busy`, `--animation-shimmer` | The spinner, and the skeleton's shimmer. |

- Animate only `opacity` and `transform`. They move nothing else on the page (ANI-03), and the browser animates them without layout, at full frame rate. With `allow-discrete`, `display` and `overlay` change at the end of a transition, so that something stays drawn while it fades out; `@starting-style` is the state it fades in from.
- Nothing waits for an animation (ANI-02). The state changes at once, and the animation only shows it. What is going away takes no pointer input (`pointer-events: none`), and neither do view transitions.
- Nothing flashes more than three times per second (ANI-07). Endless animations take at least a second per cycle.
- Status changes, such as a device going online or a new Vim mode, cross-fade in `--duration-short` rather than move.
- In a remote session, only the Vim mode indicator, the hint overlay, the toolbar showing or hiding, and notifications may animate over the remote screen (ANI-06). The remote screen itself never animates.

### Reduced motion

With reduced motion, nothing slides or grows: what appears or goes away only fades, in `--duration-short`. Skeletons don't shimmer, the spinner pulses instead of turning, and screens and the theme change without a cross-fade (ANI-04, THM-03). The tokens do this, so a component that takes its values from them needs no code for it.

Motion is reduced when the motion option, in Settings → Appearance, is Reduced, or System while the operating system or browser asks for less motion (ANI-05). `Motion` (in `src/app/shared/motion/`) holds the option as `Theme` holds the theme: it sets `data-motion="full"` or `"reduced"` on `<html>`, which `styles.scss` reads together with `prefers-reduced-motion`. It remembers the option in this browser, other tabs follow, and `appearance.js` applies it before the first paint, so that the splash screen follows it (SPL-07). Saving it in the user's preferences comes with sign-in, as for the theme. Changes to it are not audited (AUD-13).

To try it, choose Reduced in Settings, or keep System and emulate `prefers-reduced-motion: reduce` in the browser's developer tools. In specs, `animate.enter` and `animate.leave` are off, which is TestBed's default, so elements go away at once.

## Text and translation

The interface is in English, and all text must be translatable later without code changes (A11Y-04). We use Angular's built-in i18n:

- In templates, mark text with `i18n`, and attributes that people read or hear (`aria-label`, `title`, `alt`, `placeholder`) with `i18n-<attribute>`. Add `i18n="meaning|description"` when a translator would need context.
- In TypeScript, write text as `` $localize`…` ``, including route titles.
- Put templates in `.html` files, not inline in components.
- Don't translate the product name. Use `PRODUCT_NAME` from `src/app/product.ts`; `npm run check:i18n` fails on text that spells it out.

`npm run check:i18n` fails on template text that isn't marked, and `npx ng extract-i18n` writes the marked text to the file that translators work on. CI runs both. Adding a language later means a translation file and a locale in `angular.json`, with one build per language.

## Dates, times and numbers

Use the `localeDate` pipe (`'date'`, `'time'` or `'dateTime'`) and the `localeNumber` pipe. They follow the browser's locale and time zone (A11Y-04), while the interface itself stays in English. Angular's `date`, `number`, `percent` and `currency` pipes would format for the interface language instead. The server sends times in UTC (DB-14).

## Narrow windows

Every screen except the remote session view must be usable from 360 px wide (UI-07). Lay out with wrapping and relative widths, and let long values such as email addresses wrap (`overflow-wrap: anywhere`). The remote session view is designed for windows 1024 px wide or more.

## No third-party code

The web client loads nothing from other origins: no fonts, scripts, images or tracking code (PRIV-05). It uses the system's fonts. Besides the server's Content-Security-Policy, CI checks that the built `index.html` and styles reference no other origin.

## Content Security Policy

The server lets only scripts from the TwoViewer origin run in the web client (SEC-10, `server/src/http.rs`), so the built `index.html` must have no inline scripts or event handlers. CI checks this after `npm run build`. That is why the production configuration in `angular.json` turns off critical CSS inlining (`inlineCritical`): it adds an inline script that switches the stylesheet from `media="print"` to all media, which the policy blocks, leaving the page unstyled. Inline styles are allowed, because Angular adds component styles as `<style>` elements.

## Known vulnerabilities

`npm audit` runs on every change (SEC-15). `package.json` overrides the `vite` version of `@angular/build`, because `@angular/build` 22.2 pins a vite version with known vulnerabilities. Remove the override once a stable `@angular/build` 22.3 or later is out.

## Running end-to-end tests

For end-to-end (e2e) testing, run:

```bash
ng e2e
```

Angular CLI does not come with an end-to-end testing framework by default. You can choose one that suits your needs.

## Additional Resources

For more information on using the Angular CLI, including detailed command references, visit the [Angular CLI Overview and Command Reference](https://angular.dev/tools/cli) page.
