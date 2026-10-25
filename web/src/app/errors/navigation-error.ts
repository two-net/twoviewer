import { inject } from '@angular/core';
import { NavigationError, RedirectCommand, Router } from '@angular/router';

/**
 * Shows the Offline or Unavailable page when a navigation fails, for example because a screen's
 * code cannot be downloaded. The address bar keeps the address the user wanted, so *Retry* and
 * reloading go there.
 */
export function showErrorPage(error: NavigationError): RedirectCommand {
  // For developers. The page itself shows no details (UI-06).
  console.error(error.error);
  const page = navigator.onLine ? '/unavailable' : '/offline';
  return new RedirectCommand(inject(Router).parseUrl(page), { browserUrl: error.url });
}
