import { HttpErrorResponse } from '@angular/common/http';
import { UserLocale } from '../format/user-locale';
import { UserError } from './user-error';

/**
 * A `429 Too Many Requests` as the user reads it: `summary`, and when to try again, in the user's
 * time, from the response's `Retry-After` header. Without a usable header, it says to try again
 * later.
 */
export function tryAgainAfter(
  response: HttpErrorResponse,
  summary: string,
  locale: UserLocale,
  now = Date.now(),
): UserError {
  const seconds = Number(response.headers.get('Retry-After'));
  if (!Number.isFinite(seconds) || seconds <= 0) {
    return { summary, action: $localize`Try again later.` };
  }
  const time = new Intl.DateTimeFormat(locale.locales, {
    timeStyle: 'short',
    timeZone: locale.timeZone,
  }).format(now + seconds * 1000);
  return { summary, action: $localize`Try again after ${time}:time:.` };
}
