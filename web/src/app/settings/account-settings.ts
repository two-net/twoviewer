import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { Component, ElementRef, Injector, afterNextRender, inject, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { NonNullableFormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
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
import { DISPLAY_NAME_MAX_LENGTH, VERIFICATION_LINK_HOURS } from '../accounts/account-rules';
import { ServerConnection } from '../shared/connection/server-connection';
import { ErrorMessage } from '../shared/errors/error-message';
import { tryAgainAfter } from '../shared/errors/retry-after';
import { USER_LOCALE } from '../shared/format/user-locale';
import { CANCELLED, Reauthentication } from '../shared/reauthentication/reauthentication';
import { Skeleton } from '../shared/skeleton/skeleton';
import { Toasts } from '../shared/toast/toasts';

/** The signed-in user's account, as `GET /api/account` answers (`server/src/accounts/account.rs`). */
export interface Account {
  readonly id: string;
  readonly email: string;
  readonly displayName: string;
  readonly emailVerified: boolean;
  /** The address the user asked to change theirs to, while the link that confirms it works. */
  readonly pendingEmail?: string;
}

/** The real-time event that says the account has changed. */
export const ACCOUNT_CHANGED = 'account.changed';

type Field = 'displayName' | 'email';

/** Why the server didn't accept a field (`server/src/api.rs`). */
interface FieldError {
  readonly code: string;
  readonly min?: number;
  readonly max?: number;
}

/** A display name that isn't only spaces. */
function notBlank(control: { value: string }): { required: true } | null {
  return control.value.trim() === '' ? { required: true } : null;
}

/**
 * The user's name and email address, in Settings → Account (AUTH-11).
 *
 * The name changes at once. A new email address gets a link, and the address changes only once the
 * link is opened; until then, the change shows as waiting. Asking for it needs a recent
 * authentication (AUTH-10), which {@link Reauthentication} asks for when the server wants it. The
 * server answers the same whether or not the new address has an account, and so does this page.
 */
@Component({
  selector: 'app-account-settings',
  imports: [ReactiveFormsModule, ErrorMessage, Skeleton],
  templateUrl: './account-settings.html',
  styleUrls: ['../accounts/account-form.scss', './account-settings.scss'],
})
export class AccountSettings {
  private readonly http = inject(HttpClient);
  private readonly reauthentication = inject(Reauthentication);
  private readonly toasts = inject(Toasts);
  private readonly locale = inject(USER_LOCALE);
  private readonly injector = inject(Injector);
  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  private readonly formBuilder = inject(NonNullableFormBuilder);

  protected readonly linkHours = VERIFICATION_LINK_HOURS;

  /** The account, once loaded. */
  protected readonly account = signal<Account | undefined>(undefined);
  /** Why loading the account failed, until it loads. */
  protected readonly loadError = signal<unknown>(null);

  protected readonly nameForm = this.formBuilder.group({
    displayName: ['', [notBlank, Validators.maxLength(DISPLAY_NAME_MAX_LENGTH)]],
  });
  protected readonly emailForm = this.formBuilder.group({
    email: ['', [Validators.required, Validators.email]],
  });

  /** Which form was submitted, after which its field shows its errors. */
  private readonly submitted = signal<Partial<Record<Field, boolean>>>({});
  /** Why the server didn't accept fields, until they change. */
  private readonly serverErrors = signal<Partial<Record<Field, FieldError>>>({});
  protected readonly busy = signal<Field | null>(null);
  /** Why saving the name failed, until the next try. */
  protected readonly nameError = signal<unknown>(null);
  /** Why asking for a new address failed, until the next try. */
  protected readonly emailError = signal<unknown>(null);

  private readonly reloads = new Subject<void>();

  constructor() {
    const connection = inject(ServerConnection);
    // The latest answer wins: a reload cancels the one before.
    merge(
      this.reloads,
      connection.resyncs,
      connection.events.pipe(filter((event) => event.name === ACCOUNT_CHANGED)),
    )
      .pipe(
        startWith(undefined),
        switchMap(() =>
          this.http.get<Account>('/api/account').pipe(
            map((account) => ({ account, error: null })),
            catchError((error: unknown) => of({ account: undefined, error })),
          ),
        ),
        takeUntilDestroyed(),
      )
      .subscribe(({ account, error }) => {
        if (account) {
          this.account.set(account);
          // Not over a name being typed.
          if (!this.nameForm.dirty) {
            this.nameForm.reset({ displayName: account.displayName });
          }
        }
        this.loadError.set(error);
      });

    // A field's answer from the server applies until the field changes.
    this.nameForm.controls.displayName.valueChanges.subscribe(() =>
      this.clearServerError('displayName'),
    );
    this.emailForm.controls.email.valueChanges.subscribe(() => this.clearServerError('email'));
  }

  /** The error to show for `field`, if any. */
  protected errorFor(field: Field): string | null {
    const control =
      field === 'displayName' ? this.nameForm.controls.displayName : this.emailForm.controls.email;
    const server = this.serverErrors()[field];
    if (server) {
      return this.describe(field, server);
    }
    if (!control.invalid || !(control.touched || this.submitted()[field])) {
      return null;
    }
    const errors = control.errors ?? {};
    if (errors['required']) {
      return this.describe(field, { code: 'required' });
    }
    if (errors['maxlength']) {
      return this.describe(field, { code: 'too_long', max: errors['maxlength'].requiredLength });
    }
    return this.describe(field, { code: 'invalid' });
  }

  private describe(field: Field, error: FieldError): string {
    switch (`${field}.${error.code}`) {
      case 'displayName.required':
        return $localize`Enter your name.`;
      case 'displayName.too_long':
        return $localize`Use at most ${error.max}:max: characters.`;
      case 'displayName.invalid':
        return $localize`Use letters, numbers, spaces and punctuation only.`;
      case 'email.required':
        return $localize`Enter your new email address.`;
      case 'email.invalid':
        return $localize`Enter an email address such as name@example.com.`;
      case 'email.unchanged':
        return $localize`This is your email address already. Enter another one.`;
      default:
        return $localize`Check this field.`;
    }
  }

  /** Saves the name. */
  protected async saveName(): Promise<void> {
    if (this.busy()) {
      return;
    }
    this.submitted.update((submitted) => ({ ...submitted, displayName: true }));
    this.nameError.set(null);
    if (this.nameForm.invalid) {
      this.focusAfterRender('#settings-display-name');
      return;
    }
    this.busy.set('displayName');
    try {
      const { displayName } = this.nameForm.getRawValue();
      await firstValueFrom(this.http.patch('/api/account', { displayName }));
      this.nameForm.markAsPristine();
      this.submitted.update(({ displayName: _, ...others }) => others);
      this.toasts.show($localize`Name saved.`);
      this.reloads.next();
    } catch (error) {
      if (!this.fieldErrors(error, '#settings-display-name')) {
        this.nameError.set(error);
      }
    } finally {
      this.busy.set(null);
    }
  }

  /** Emails a link to the new address, which changes the account's to it once it is opened. */
  protected async changeEmail(): Promise<void> {
    if (this.busy()) {
      return;
    }
    this.submitted.update((submitted) => ({ ...submitted, email: true }));
    this.emailError.set(null);
    if (this.emailForm.invalid) {
      this.focusAfterRender('#settings-new-email');
      return;
    }
    this.busy.set('email');
    try {
      const email = this.emailForm.getRawValue().email.trim();
      const answer = await this.reauthentication.run(() =>
        this.http.post('/api/account/email', { email }),
      );
      if (answer !== CANCELLED) {
        this.emailForm.reset();
        this.submitted.update(({ email: _, ...others }) => others);
        this.toasts.show($localize`We sent an email to ${email}:address:.`);
        this.reloads.next();
      }
    } catch (error) {
      if (error instanceof HttpErrorResponse && error.status === 429) {
        this.emailError.set(
          tryAgainAfter(
            error,
            $localize`You have asked to change your email address too often.`,
            this.locale,
          ),
        );
      } else if (!this.fieldErrors(error, '#settings-new-email')) {
        this.emailError.set(error);
      }
    } finally {
      this.busy.set(null);
    }
  }

  /** Shows the fields that a `422` names, and focuses `selector`. Whether `error` was one. */
  private fieldErrors(error: unknown, selector: string): boolean {
    if (!(error instanceof HttpErrorResponse) || error.status !== 422) {
      return false;
    }
    this.serverErrors.update((errors) => ({ ...errors, ...(error.error?.fields ?? {}) }));
    this.focusAfterRender(selector);
    return true;
  }

  private clearServerError(field: Field): void {
    this.serverErrors.update(({ [field]: _, ...others }) => others);
  }

  private focusAfterRender(selector: string): void {
    afterNextRender(() => this.host.nativeElement.querySelector<HTMLElement>(selector)?.focus(), {
      injector: this.injector,
    });
  }
}
