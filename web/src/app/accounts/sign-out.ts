import { HttpClient } from '@angular/common/http';
import { Injectable, inject } from '@angular/core';
import { Router } from '@angular/router';
import { firstValueFrom } from 'rxjs';
import { ServerConnection } from '../shared/connection/server-connection';
import { Toasts } from '../shared/toast/toasts';

/** Where the browser goes once its sign-in has ended. */
export const SIGN_IN_PAGE = '/sign-in';

/**
 * The end of this browser's sign-in (AUTH-09): when the user signs out, and when the server says
 * that the sign-in has ended otherwise, because it was revoked or has expired. Either way the
 * real-time connection stops, and the browser goes to the sign-in page.
 */
@Injectable({ providedIn: 'root' })
export class SignOut {
  private readonly http = inject(HttpClient);
  private readonly router = inject(Router);
  private readonly connection = inject(ServerConnection);
  private readonly toasts = inject(Toasts);

  /** While signing out, the server's word that the sign-in has ended is expected. */
  private signingOut = false;

  /**
   * Ends this browser's sign-in, then goes to the sign-in page. Rejects if the server doesn't
   * answer, in which case the browser is still signed in.
   */
  async signOut(): Promise<void> {
    this.signingOut = true;
    try {
      // The server removes the cookie, which page scripts can't.
      await firstValueFrom(this.http.post('/api/sign-out', null));
      await this.ended();
    } finally {
      this.signingOut = false;
    }
  }

  /**
   * The sign-in has ended: stops the real-time connection and goes to the sign-in page, telling
   * the user `notice` if there is one, unless they are signing out themselves.
   */
  async ended(notice?: string): Promise<void> {
    this.connection.stop();
    if (notice && !this.signingOut) {
      this.toasts.show(notice);
    }
    await this.router.navigateByUrl(SIGN_IN_PAGE);
  }
}
