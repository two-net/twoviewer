import { ApplicationConfig, provideBrowserGlobalErrorListeners } from '@angular/core';
import { TitleStrategy, provideRouter, withNavigationErrorHandler } from '@angular/router';
import { routes } from './app.routes';
import { showErrorPage } from './errors/navigation-error';
import { PageTitle } from './layout/page-title';

export const appConfig: ApplicationConfig = {
  providers: [
    provideBrowserGlobalErrorListeners(),
    provideRouter(routes, withNavigationErrorHandler(showErrorPage)),
    { provide: TitleStrategy, useClass: PageTitle },
  ],
};
