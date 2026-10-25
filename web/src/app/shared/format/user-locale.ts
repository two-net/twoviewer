import { InjectionToken } from '@angular/core';

/**
 * The locale and time zone that dates, times and numbers follow (A11Y-04). They are the
 * browser's, while the interface itself is in English.
 */
export interface UserLocale {
  /** The user's languages, most preferred first. */
  readonly locales: readonly string[];
  /** An IANA time zone such as `Europe/Berlin`, or `undefined` for the browser's. */
  readonly timeZone?: string;
}

export const USER_LOCALE = new InjectionToken<UserLocale>('USER_LOCALE', {
  providedIn: 'root',
  factory: () => ({ locales: navigator.languages }),
});
