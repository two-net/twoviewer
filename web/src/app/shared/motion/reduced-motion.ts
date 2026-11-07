import { DOCUMENT } from '@angular/common';
import { InjectionToken, inject } from '@angular/core';

/**
 * Whether motion should be reduced right now (ANI-04). It follows the operating system's or
 * browser's preference, read at each call so that a change applies at once. Tests replace it.
 */
export const REDUCED_MOTION = new InjectionToken<() => boolean>('REDUCED_MOTION', {
  providedIn: 'root',
  factory: () => {
    const query = inject(DOCUMENT).defaultView?.matchMedia?.('(prefers-reduced-motion: reduce)');
    return () => query?.matches ?? false;
  },
});
