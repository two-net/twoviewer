import { HttpErrorResponse } from '@angular/common/http';

/** An error as the user reads it: what happened, and what they can do (UI-06). */
export interface UserError {
  readonly summary: string;
  readonly action: string;
}

/**
 * Describes an error for the user, from the browser's connection and the HTTP status alone.
 * Server messages, status codes and stack traces never reach the user. Features describe the
 * errors they expect themselves, and use this for the rest.
 */
export function describeError(error: unknown, online = navigator.onLine): UserError {
  if (!online) {
    return {
      summary: $localize`You're offline.`,
      action: $localize`Check your internet connection and try again.`,
    };
  }
  switch (error instanceof HttpErrorResponse ? error.status : undefined) {
    case 0:
    case 502:
    case 503:
    case 504:
      return {
        summary: $localize`TwoViewer can't be reached right now.`,
        action: $localize`Try again in a few minutes.`,
      };
    case 400:
    case 422:
      return {
        summary: $localize`Some of the information isn't valid.`,
        action: $localize`Check what you entered and try again.`,
      };
    case 401:
      return {
        summary: $localize`You're no longer signed in.`,
        action: $localize`Sign in again to continue.`,
      };
    case 403:
      return {
        summary: $localize`Your account can't do this.`,
        action: $localize`Check that you're signed in with the right account.`,
      };
    case 404:
      return {
        summary: $localize`This no longer exists.`,
        action: $localize`It may have been removed. Go back and try again.`,
      };
    case 409:
      return {
        summary: $localize`This was changed in the meantime.`,
        action: $localize`Reload the page to see the latest state, then try again.`,
      };
    case 413:
      return {
        summary: $localize`This is too large.`,
        action: $localize`Try again with something smaller.`,
      };
    case 429:
      return {
        summary: $localize`Too many attempts.`,
        action: $localize`Wait a few minutes, then try again.`,
      };
    default:
      return {
        summary: $localize`Something went wrong.`,
        action: $localize`Try again. If it keeps happening, contact support.`,
      };
  }
}
