# TwoviewerWeb

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

- `layout/app-layout`: the signed-in screens, with the main navigation. Below 48rem wide, the navigation opens from a *Menu* button.
- `layout/plain-layout`: the visitor screens, the host page, the policies and the error pages, with the product name only.

The remote session view has no layout and fills the window. The root component, [`src/app/app.html`](src/app/app.html), adds what every page has: a skip link, the footer with the Terms of Service and Privacy Notice (PRIV-04), the toasts and the confirmation dialog. `/terms` and `/privacy` are placeholders until the texts are written.

Unknown addresses show *Page not found*. When a screen can't be loaded, for example because its code can't be downloaded, `errors/navigation-error.ts` shows *You're offline* or *This page couldn't be loaded* and keeps the address in the address bar. *Retry* loads that address as a new page, because browsers remember a module that failed to download, and only a new page downloads it again. The offline page has no *Retry*: it loads the page on its own once the browser is back online. Both pages are bundled with the app rather than downloaded later, so they can still be shown when downloads fail.

## Shared components

In [`src/app/shared/`](src/app/shared):

| Use | For |
|---|---|
| `Confirmation.confirm({ title, message, confirmLabel, typeToConfirm? })` | Destructive actions (UI-03). It resolves to `true` if the user confirms. With `typeToConfirm`, such as the account's email address, the user must type it first. |
| `<app-empty-state>` | A list with nothing in it. Say what the next step is, and link to it with an element marked `actions` (UI-04). |
| `describeError(error)`, `<app-error-message [error]>` | Errors: what happened and what the user can do, never server messages or status codes (UI-06). Describe the errors a feature expects yourself, and use these for the rest. |
| `Toasts.show(message)` | Confirming that a background action has finished (UI-08). A toast disappears after 5 seconds and pauses while hovered or focused. The 5 seconds is a constant, not a server setting (MNT-01), until the web client gets settings from the server. |
| `localeDate`, `localeNumber` pipes | Dates, times and numbers ([below](#dates-times-and-numbers)). |

Style with the color tokens in [`src/styles.scss`](src/styles.scss) (`var(--color-text)` and so on), never fixed colors, so that every component works in both themes (THM-06). The colors are provisional until the palettes are designed (#19).

## Text and translation

The interface is in English, and all text must be translatable later without code changes (A11Y-04). We use Angular's built-in i18n:

- In templates, mark text with `i18n`, and attributes that people read or hear (`aria-label`, `title`, `alt`, `placeholder`) with `i18n-<attribute>`. Add `i18n="meaning|description"` when a translator would need context.
- In TypeScript, write text as `` $localize`…` ``, including route titles.
- Put templates in `.html` files, not inline in components.
- Don't translate the product name. Use `PRODUCT_NAME` from `src/app/product.ts`.

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
