import { HttpErrorResponse, HttpHeaders } from '@angular/common/http';
import { tryAgainAfter } from './retry-after';

describe('tryAgainAfter', () => {
  const locale = { locales: ['en-GB'], timeZone: 'UTC' };
  const now = Date.UTC(2026, 10, 8, 9, 30);

  function tooMany(retryAfter?: string): HttpErrorResponse {
    return new HttpErrorResponse({
      status: 429,
      headers:
        retryAfter === undefined ? undefined : new HttpHeaders({ 'Retry-After': retryAfter }),
    });
  }

  it("says when to try again, in the user's time", () => {
    expect(tryAgainAfter(tooMany('900'), 'Too many.', locale, now)).toEqual({
      summary: 'Too many.',
      action: 'Try again after 09:45.',
    });
  });

  it.each([undefined, '', '0', '-5', 'Wed, 21 Oct 2015 07:28:00 GMT'])(
    'says to try again later without a usable Retry-After (%s)',
    (retryAfter) => {
      expect(tryAgainAfter(tooMany(retryAfter), 'Too many.', locale, now).action).toBe(
        'Try again later.',
      );
    },
  );
});
