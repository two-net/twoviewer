import { ApplicationConfig, provideBrowserGlobalErrorListeners } from '@angular/core';
import {
  TitleStrategy,
  provideRouter,
  withNavigationErrorHandler,
  withViewTransitions,
} from '@angular/router';
import { routes } from './app.routes';
import { showErrorPage } from './errors/navigation-error';
import { PageTitle } from './layout/page-title';
import { screenTransition } from './shared/motion/screen-transition';

export const appConfig: ApplicationConfig = {
  providers: [
    provideBrowserGlobalErrorListeners(),
    provideRouter(
      routes,
      withNavigationErrorHandler(showErrorPage),
      // Not on the first screen of a page load, which the splash screen leads to (SPL-04).
      withViewTransitions({
        skipInitialTransition: true,
        onViewTransitionCreated: screenTransition,
      }),
    ),
    { provide: TitleStrategy, useClass: PageTitle },
  ],
};
