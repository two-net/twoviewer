import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import {
  Component,
  ElementRef,
  Injector,
  afterNextRender,
  inject,
  signal,
  viewChild,
} from '@angular/core';
import {
  AbstractControl,
  NonNullableFormBuilder,
  ReactiveFormsModule,
  Validators,
} from '@angular/forms';
import { RouterLink } from '@angular/router';
import { firstValueFrom } from 'rxjs';
import { PRIVACY_VERSION, TERMS_VERSION } from '../legal/versions';
import { PRODUCT_NAME } from '../product';
import { ErrorMessage } from '../shared/errors/error-message';
import { UserError } from '../shared/errors/user-error';
import { USER_LOCALE } from '../shared/format/user-locale';
import {
  DISPLAY_NAME_MAX_LENGTH,
  MIN_PASSWORD_LENGTH,
  VERIFICATION_LINK_HOURS,
} from './account-rules';
import { Challenge, ProofOfWork, Solution } from './proof-of-work';

type Field = 'email' | 'displayName' | 'password' | 'terms';

/** Why the server didn't accept a field (`server/src/api.rs`). */
interface FieldError {
  readonly code: string;
  readonly min?: number;
  readonly max?: number;
}

/** How often a sign-up is sent with a new challenge solved, before giving up. */
const MAX_CHALLENGES = 3;

/** A display name that isn't only spaces. */
function notBlank(control: { value: string }): { required: true } | null {
  return control.value.trim() === '' ? { required: true } : null;
}

/**
 * Sign-up (AUTH-01): email address, display name and password, and acceptance of the Terms of
 * Service and Privacy Notice, whose versions are sent along.
 *
 * The server answers the same whether or not the address is registered (SEC-14), so the page
 * always says to check the email. When the network has signed up often, the server asks for a
 * proof-of-work challenge first (AUTH-15), which the page solves and sends with the sign-up again.
 */
@Component({
  selector: 'app-sign-up',
  imports: [ReactiveFormsModule, RouterLink, ErrorMessage],
  templateUrl: './sign-up.html',
  styleUrl: './sign-up.scss',
})
export class SignUp {
  private readonly http = inject(HttpClient);
  private readonly proofOfWork = inject(ProofOfWork);
  private readonly locale = inject(USER_LOCALE);
  private readonly injector = inject(Injector);
  private readonly formElement = viewChild<ElementRef<HTMLFormElement>>('formElement');
  private readonly sentHeading = viewChild<ElementRef<HTMLElement>>('sentHeading');

  protected readonly productName = PRODUCT_NAME;
  protected readonly minPasswordLength = MIN_PASSWORD_LENGTH;
  protected readonly linkHours = VERIFICATION_LINK_HOURS;

  protected readonly form = inject(NonNullableFormBuilder).group({
    email: ['', [Validators.required, Validators.email]],
    displayName: ['', [notBlank, Validators.maxLength(DISPLAY_NAME_MAX_LENGTH)]],
    password: ['', [Validators.required, Validators.minLength(MIN_PASSWORD_LENGTH)]],
    terms: [false, Validators.requiredTrue],
  });

  /** Whether the form was submitted, after which every field shows its errors. */
  private readonly submitted = signal(false);
  /** Why the server didn't accept fields, until they change. */
  private readonly serverErrors = signal<Partial<Record<Field, FieldError>>>({});
  protected readonly busy = signal(false);
  /** Whether a challenge is being solved. */
  protected readonly checking = signal(false);
  protected readonly error = signal<unknown>(null);
  /** The address the link was sent to, once it was. */
  protected readonly sentTo = signal<string | null>(null);

  constructor() {
    // A field's answer from the server applies until the field changes.
    for (const field of Object.keys(this.form.controls) as Field[]) {
      const control: AbstractControl = this.form.controls[field];
      control.valueChanges.subscribe(() =>
        this.serverErrors.update(({ [field]: _, ...others }) => others),
      );
    }
  }

  /** The error to show for `field`, if any. */
  protected errorFor(field: Field): string | null {
    const control = this.form.controls[field];
    const server = this.serverErrors()[field];
    if (server) {
      return this.describe(field, server);
    }
    if (!control.invalid || !(control.touched || this.submitted())) {
      return null;
    }
    const errors = control.errors ?? {};
    if (errors['required']) {
      return this.describe(field, { code: 'required' });
    }
    if (errors['email']) {
      return this.describe(field, { code: 'invalid' });
    }
    if (errors['minlength']) {
      return this.describe(field, { code: 'too_short', min: errors['minlength'].requiredLength });
    }
    if (errors['maxlength']) {
      return this.describe(field, { code: 'too_long', max: errors['maxlength'].requiredLength });
    }
    return this.describe(field, { code: 'invalid' });
  }

  private describe(field: Field, error: FieldError): string {
    switch (`${field}.${error.code}`) {
      case 'email.required':
        return $localize`Enter your email address.`;
      case 'email.invalid':
        return $localize`Enter an email address such as name@example.com.`;
      case 'displayName.required':
        return $localize`Enter your name.`;
      case 'displayName.too_long':
        return $localize`Use at most ${error.max}:max: characters.`;
      case 'displayName.invalid':
        return $localize`Use letters, numbers, spaces and punctuation only.`;
      case 'password.required':
        return $localize`Enter a password.`;
      case 'password.too_short':
        return $localize`Use at least ${error.min}:min: characters.`;
      case 'password.too_long':
        return $localize`Use at most ${error.max}:max: characters.`;
      case 'password.compromised':
        return $localize`This password is on a list of passwords stolen from other sites. Choose another one.`;
      case 'terms.required':
        return $localize`Accept the Terms of Service and the Privacy Notice to sign up.`;
      default:
        return $localize`Check this field.`;
    }
  }

  protected async submit(): Promise<void> {
    if (this.busy()) {
      return;
    }
    this.submitted.set(true);
    this.error.set(null);
    if (this.form.invalid) {
      afterNextRender(() => this.focusFirstError(), { injector: this.injector });
      return;
    }
    this.busy.set(true);
    try {
      await this.send();
    } finally {
      this.busy.set(false);
      this.checking.set(false);
    }
  }

  /** Sends the sign-up, solving up to `MAX_CHALLENGES` challenges on the way. */
  private async send(): Promise<void> {
    const { email, displayName, password } = this.form.getRawValue();
    let challenge: Solution | undefined;
    for (let attempt = 0; attempt <= MAX_CHALLENGES; attempt++) {
      try {
        await firstValueFrom(
          this.http.post('/api/sign-up', {
            email,
            displayName,
            password,
            termsVersion: TERMS_VERSION,
            privacyVersion: PRIVACY_VERSION,
            challenge,
          }),
        );
        this.sent(email.trim());
        return;
      } catch (error) {
        const status = error instanceof HttpErrorResponse ? error.status : undefined;
        if (status === 428 && attempt < MAX_CHALLENGES) {
          this.checking.set(true);
          challenge = await this.proofOfWork.solve(
            (error as HttpErrorResponse).error.challenge as Challenge,
          );
          continue;
        }
        this.failed(error);
        return;
      }
    }
  }

  private sent(address: string): void {
    this.sentTo.set(address);
    this.form.controls.password.reset();
    afterNextRender(() => this.sentHeading()?.nativeElement.focus(), { injector: this.injector });
  }

  private failed(error: unknown): void {
    const response = error instanceof HttpErrorResponse ? error : undefined;
    switch (response?.status) {
      case 422:
        this.serverErrors.set(response.error?.fields ?? {});
        afterNextRender(() => this.focusFirstError(), { injector: this.injector });
        return;
      case 409:
        this.error.set({
          summary: $localize`The Terms of Service or the Privacy Notice have changed.`,
          action: $localize`Reload the page to read them, then sign up.`,
        } satisfies UserError);
        return;
      case 429:
        this.error.set(this.tooMany(response));
        return;
      default:
        this.error.set(error);
    }
  }

  /** Too many sign-ups from this network: says when to try again, from `Retry-After`. */
  private tooMany(response: HttpErrorResponse): UserError {
    const seconds = Number(response.headers.get('Retry-After'));
    const summary = $localize`Too many sign-ups have come from your network.`;
    if (!Number.isFinite(seconds) || seconds <= 0) {
      return { summary, action: $localize`Try again later.` };
    }
    const time = new Intl.DateTimeFormat(this.locale.locales, {
      timeStyle: 'short',
      timeZone: this.locale.timeZone,
    }).format(Date.now() + seconds * 1000);
    return { summary, action: $localize`Try again after ${time}:time:.` };
  }

  private focusFirstError(): void {
    this.formElement()?.nativeElement.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus();
  }

  /** Back to the form, to sign up again, for example to get a new link. */
  protected startOver(): void {
    this.sentTo.set(null);
    this.submitted.set(false);
  }
}
