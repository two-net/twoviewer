import { Location } from '@angular/common';
import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { Component, inject, signal } from '@angular/core';
import { ActivatedRoute, RouterLink } from '@angular/router';
import { firstValueFrom } from 'rxjs';
import { ErrorMessage } from '../shared/errors/error-message';
import { VERIFICATION_LINK_HOURS } from './account-rules';

type State = 'verifying' | 'verified' | 'expired' | 'missing' | 'failed';

/**
 * Where the link in the verification email leads (AUTH-02): `/verify-email#<token>`. The token is
 * in the fragment, which browsers never send to a server, so it stays out of server logs and the
 * `Referer` header. The page takes it out of the address bar and sends it to the server.
 */
@Component({
  selector: 'app-verify-email',
  imports: [RouterLink, ErrorMessage],
  templateUrl: './verify-email.html',
})
export class VerifyEmail {
  private readonly http = inject(HttpClient);
  private readonly token: string | null;

  protected readonly linkHours = VERIFICATION_LINK_HOURS;
  protected readonly state = signal<State>('verifying');
  protected readonly error = signal<unknown>(null);

  constructor() {
    this.token = inject(ActivatedRoute).snapshot.fragment || null;
    // Out of the address bar and the history, where others could find it.
    inject(Location).replaceState('/verify-email');
    void this.verify();
  }

  protected async verify(): Promise<void> {
    if (!this.token) {
      this.state.set('missing');
      return;
    }
    this.state.set('verifying');
    try {
      await firstValueFrom(this.http.post('/api/verify-email', { token: this.token }));
      this.state.set('verified');
    } catch (error) {
      if (error instanceof HttpErrorResponse && error.status === 410) {
        this.state.set('expired');
      } else {
        this.error.set(error);
        this.state.set('failed');
      }
    }
  }
}
