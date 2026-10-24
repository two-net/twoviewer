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
