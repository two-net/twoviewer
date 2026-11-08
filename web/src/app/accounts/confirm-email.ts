import { Location } from '@angular/common';
import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { Component, inject, signal } from '@angular/core';
import { ActivatedRoute, RouterLink } from '@angular/router';
import { firstValueFrom } from 'rxjs';
import { ErrorMessage } from '../shared/errors/error-message';
import { VERIFICATION_LINK_HOURS } from './account-rules';

type State = 'confirming' | 'confirmed' | 'expired' | 'in_use' | 'missing' | 'failed';

/**
 * Where the link in the email to a new address leads (AUTH-11): `/confirm-email#<token>`. Opening
 * it changes the account's address to that one. As with email verification, the token is in the
 * fragment, which browsers never send to a server, and the page takes it out of the address bar.
 * It works without a sign-in, since the link may be opened on another device.
 */
@Component({
  selector: 'app-confirm-email',
  imports: [RouterLink, ErrorMessage],
  templateUrl: './confirm-email.html',
  styleUrl: './account-form.scss',
})
export class ConfirmEmail {
  private readonly http = inject(HttpClient);
  private readonly token: string | null;

  protected readonly linkHours = VERIFICATION_LINK_HOURS;
  protected readonly state = signal<State>('confirming');
  /** The account's new address, once it has changed. */
  protected readonly email = signal('');
  protected readonly error = signal<unknown>(null);

  constructor() {
    this.token = inject(ActivatedRoute).snapshot.fragment || null;
    // Out of the address bar and the history, where others could find it.
    inject(Location).replaceState('/confirm-email');
    void this.confirm();
  }

  protected async confirm(): Promise<void> {
    if (!this.token) {
      this.state.set('missing');
      return;
    }
    this.state.set('confirming');
    try {
      const answer = await firstValueFrom(
        this.http.post<{ email: string }>('/api/account/email/confirm', { token: this.token }),
      );
      this.email.set(answer.email);
      this.state.set('confirmed');
    } catch (error) {
      if (error instanceof HttpErrorResponse && error.status === 410) {
        this.state.set('expired');
      } else if (error instanceof HttpErrorResponse && error.status === 409) {
        this.state.set('in_use');
      } else {
        this.error.set(error);
        this.state.set('failed');
      }
    }
  }
}
