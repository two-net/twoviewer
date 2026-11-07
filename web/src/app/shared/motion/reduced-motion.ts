import { InjectionToken, inject } from '@angular/core';
import { Motion } from './motion';

/**
 * Whether motion should be reduced right now (ANI-04), for code that starts animations itself,
 * such as view transitions. It follows the motion option, and with System the operating system's
 * or browser's preference, read at each call so that a change applies at once. Tests replace it.
 */
export const REDUCED_MOTION = new InjectionToken<() => boolean>('REDUCED_MOTION', {
  providedIn: 'root',
  factory: () => {
    const motion = inject(Motion);
    return () => motion.reduced();
  },
});
