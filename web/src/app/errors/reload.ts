import { DOCUMENT } from '@angular/common';
import { InjectionToken, inject } from '@angular/core';

/**
 * Loads the address in the address bar, which {@link showErrorPage} keeps, as a new page. A
 * navigation in the app would fail again: browsers remember a module that failed to download,
 * and only a new page downloads it again.
 */
export const RELOAD = new InjectionToken<() => void>('RELOAD', {
  providedIn: 'root',
  factory: () => {
    const { history, location } = inject(DOCUMENT).defaultView!;
    return () => {
      // The history entry remembers that the router showed the error page there, and the
      // router would show it again after the reload.
      history.replaceState(null, '');
      location.reload();
    };
  },
});
