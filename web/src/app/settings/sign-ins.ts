import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { Component, computed, inject, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import {
  Subject,
  catchError,
  filter,
  firstValueFrom,
  map,
  merge,
  of,
  startWith,
  switchMap,
} from 'rxjs';
import { SignOut } from '../accounts/sign-out';
import { Confirmation } from '../shared/confirm/confirmation';
import { ServerConnection } from '../shared/connection/server-connection';
import { ErrorMessage } from '../shared/errors/error-message';
import { LocaleDatePipe } from '../shared/format/locale-date-pipe';
import { Skeleton } from '../shared/skeleton/skeleton';
import { Toasts } from '../shared/toast/toasts';

/** An active sign-in, as `GET /api/sign-ins` describes it (`server/src/accounts/sign_ins.rs`). */
export interface SignIn {
  readonly id: string;
  /** Browser and operating system, such as "Firefox on Linux". */
  readonly client: string;
  /** Where the user signed in from. */
  readonly ipAddress: string;
  readonly signedInAt: string;
  /** To the minute. */
  readonly lastActiveAt: string;
  /** Whether it is this browser's sign-in. */
  readonly current: boolean;
}

/** The real-time event that says the user's sign-ins have changed. */
export const SIGN_INS_CHANGED = 'sign_ins.changed';

/** What {@link SignIns.ending} is while all other sign-ins are being ended. */
const OTHERS = 'others';

/**
 * The user's active sign-ins, in Settings → Security (AUTH-09): browser and operating system, IP
 * address and last activity. The user can sign out of any of them, this browser included, or of
 * all but this browser. The list follows changes made elsewhere, such as a new sign-in.
 */
@Component({
  selector: 'app-sign-ins',
  imports: [ErrorMessage, LocaleDatePipe, Skeleton],
  templateUrl: './sign-ins.html',
  styleUrl: './sign-ins.scss',
})
export class SignIns {
  private readonly http = inject(HttpClient);
  private readonly signOut = inject(SignOut);
  private readonly confirmation = inject(Confirmation);
  private readonly toasts = inject(Toasts);

  /** The active sign-ins, this browser's first, once loaded. */
  protected readonly signIns = signal<readonly SignIn[] | undefined>(undefined);
  /** Why loading them failed, until they load. */
  protected readonly loadError = signal<unknown>(null);
  /** Why ending a sign-in failed, until the next attempt. */
  protected readonly error = signal<unknown>(null);
  /** The ID of the sign-in being ended, or {@link OTHERS}. */
  protected readonly ending = signal<string | null>(null);
  protected readonly othersCount = computed(
    () => this.signIns()?.filter((signIn) => !signIn.current).length ?? 0,
  );

  private readonly reloads = new Subject<void>();

  constructor() {
    const connection = inject(ServerConnection);
    // The latest answer wins: a reload cancels the one before.
    merge(
      this.reloads,
      connection.resyncs,
      connection.events.pipe(filter((event) => event.name === SIGN_INS_CHANGED)),
    )
      .pipe(
        startWith(undefined),
        switchMap(() =>
          this.http.get<{ signIns: SignIn[] }>('/api/sign-ins').pipe(
            map((answer) => ({ signIns: answer.signIns, error: null })),
            catchError((error: unknown) => of({ signIns: undefined, error })),
          ),
        ),
        takeUntilDestroyed(),
      )
      .subscribe(({ signIns, error }) => {
        if (signIns) {
          this.signIns.set(signIns);
        }
        this.loadError.set(error);
      });
  }

  /** The accessible name of the button that signs out of `signIn`. */
  protected signOutLabel(signIn: SignIn): string {
    return signIn.current
      ? $localize`Sign out of this browser`
      : $localize`Sign out of ${signIn.client}:client:`;
  }

  /** Ends `signIn`. For this browser's own, that is signing out. */
  protected async end(signIn: SignIn): Promise<void> {
    this.ending.set(signIn.id);
    this.error.set(null);
    try {
      if (signIn.current) {
        await this.signOut.signOut();
        return;
      }
      await firstValueFrom(this.http.delete(`/api/sign-ins/${encodeURIComponent(signIn.id)}`));
      this.toasts.show($localize`Signed out of ${signIn.client}:client:.`);
      this.reloads.next();
    } catch (error) {
      if (error instanceof HttpErrorResponse && error.status === 410) {
        // It had ended already.
        this.reloads.next();
      } else {
        this.error.set(error);
      }
    } finally {
      this.ending.set(null);
    }
  }

  /** Ends every sign-in but this browser's, once the user confirms. */
  protected async endOthers(): Promise<void> {
    const confirmed = await this.confirmation.confirm({
      title: $localize`Sign out everywhere else?`,
      message: $localize`Every other browser signed in to your account will be signed out. This browser stays signed in.`,
      confirmLabel: $localize`Sign out everywhere else`,
    });
    if (!confirmed) {
      return;
    }
    this.ending.set(OTHERS);
    this.error.set(null);
    try {
      await firstValueFrom(this.http.delete('/api/sign-ins'));
      this.toasts.show($localize`Signed out everywhere else.`);
      this.reloads.next();
    } catch (error) {
      this.error.set(error);
    } finally {
      this.ending.set(null);
    }
  }
}
