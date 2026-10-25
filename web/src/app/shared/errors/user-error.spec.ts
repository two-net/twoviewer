import { HttpErrorResponse } from '@angular/common/http';
import { describeError } from './user-error';

describe('describeError', () => {
  function http(status: number): HttpErrorResponse {
    return new HttpErrorResponse({
      status,
      error: { message: 'duplicate key value violates unique constraint "users_email_key"' },
      url: '/api/internal/thing',
    });
  }

  it('says what happened and what to do', () => {
    expect(describeError(http(429), true)).toEqual({
      summary: 'Too many attempts.',
      action: 'Wait a few minutes, then try again.',
    });
  });

  it('tells offline users to check their connection, whatever the error', () => {
    expect(describeError(http(500), false).summary).toBe("You're offline.");
  });

  it.each([0, 502, 503, 504])('treats status %i as TwoViewer being unreachable', (status) => {
    expect(describeError(http(status), true).summary).toBe("TwoViewer can't be reached right now.");
  });

  it.each([400, 401, 403, 404, 409, 413, 422, 429, 500, 501])(
    'describes status %i without server details',
    (status) => {
      const text = Object.values(describeError(http(status), true)).join(' ');

      expect(text).not.toMatch(/duplicate|users_email_key|api|internal|\d{3}/);
    },
  );

  it('describes anything else as a general failure', () => {
    expect(describeError(new TypeError('x is undefined'), true)).toEqual({
      summary: 'Something went wrong.',
      action: 'Try again. If it keeps happening, contact support.',
    });
  });
});
