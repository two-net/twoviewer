import { HttpHeaders, provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { PRIVACY_VERSION, TERMS_VERSION } from '../legal/versions';
import { Challenge, ProofOfWork, Solution } from './proof-of-work';
import { SignUp } from './sign-up';

describe('SignUp', () => {
  let fixture: ComponentFixture<SignUp>;
  let page: HTMLElement;
  let server: HttpTestingController;
  let solved: Challenge[];

  beforeEach(async () => {
    solved = [];
    TestBed.configureTestingModule({
      imports: [SignUp],
      providers: [
        provideRouter([]),
        provideHttpClient(),
        provideHttpClientTesting(),
        {
          provide: ProofOfWork,
          useValue: {
            solve: async (challenge: Challenge): Promise<Solution> => {
              solved.push(challenge);
              return { id: challenge.id, nonce: 7 };
            },
          },
        },
      ],
    });
    fixture = TestBed.createComponent(SignUp);
    page = fixture.nativeElement;
    server = TestBed.inject(HttpTestingController);
    await fixture.whenStable();
  });

  afterEach(() => server.verify());

  function input(id: string): HTMLInputElement {
    return page.querySelector<HTMLInputElement>(`#${id}`)!;
  }

  async function type(id: string, value: string): Promise<void> {
    input(id).value = value;
    input(id).dispatchEvent(new Event('input'));
    await fixture.whenStable();
  }

  async function fillIn(): Promise<void> {
    await type('sign-up-email', ' Ann@Example.com ');
    await type('sign-up-name', 'Ann Lee');
    await type('sign-up-password', 'violet otter harbor 92');
    input('sign-up-terms').click();
    await fixture.whenStable();
  }

  async function submit(): Promise<void> {
    page.querySelector<HTMLButtonElement>('button[type=submit]')!.click();
    await fixture.whenStable();
  }

  /** Lets the component handle a response, which takes a few turns of promises. */
  async function settle(): Promise<void> {
    for (let i = 0; i < 5; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      await fixture.whenStable();
    }
  }

  function errors(): string[] {
    return Array.from(page.querySelectorAll('.field-error'), (error) => error.textContent!.trim());
  }

  it('sends the sign-up with the accepted versions, then says to check the email', async () => {
    await fillIn();
    await submit();

    const request = server.expectOne('/api/sign-up');
    expect(request.request.method).toBe('POST');
    expect(request.request.body).toEqual({
      // Browsers strip the spaces around the value of an email input.
      email: 'Ann@Example.com',
      displayName: 'Ann Lee',
      password: 'violet otter harbor 92',
      termsVersion: TERMS_VERSION,
      privacyVersion: PRIVACY_VERSION,
      challenge: undefined,
    });
    request.flush(null, { status: 202, statusText: 'Accepted' });
    await settle();

    const heading = page.querySelector('h1')!;
    expect(heading.textContent).toBe('Check your email');
    expect(document.activeElement).toBe(heading);
    expect(page.querySelector('.address')!.textContent).toBe('Ann@Example.com');
    expect(page.textContent).toContain('works for 24 hours');
  });

  it('checks the fields before sending', async () => {
    await type('sign-up-password', 'short');
    await submit();

    server.expectNone('/api/sign-up');
    expect(errors()).toEqual([
      'Enter your email address.',
      'Enter your name.',
      'Use at least 12 characters.',
      'Accept the Terms of Service and the Privacy Notice to sign up.',
    ]);
    const email = input('sign-up-email');
    expect(email.getAttribute('aria-invalid')).toBe('true');
    expect(email.getAttribute('aria-describedby')).toBe('sign-up-email-error');
    await settle();
    expect(document.activeElement).toBe(email);
  });

  it("shows the server's reasons next to the fields, until they change", async () => {
    await fillIn();
    await submit();

    server.expectOne('/api/sign-up').flush(
      {
        error: 'invalid',
        fields: { password: { code: 'compromised' }, displayName: { code: 'too_long', max: 50 } },
      },
      { status: 422, statusText: 'Unprocessable Entity' },
    );
    await settle();

    expect(errors()).toEqual([
      'Use at most 50 characters.',
      'This password is on a list of passwords stolen from other sites. Choose another one.',
    ]);
    expect(document.activeElement).toBe(input('sign-up-name'));
    await type('sign-up-password', 'another long password 5');
    expect(errors()).toEqual(['Use at most 50 characters.']);
  });

  it('solves a challenge when the server asks for one, and sends the sign-up again', async () => {
    await fillIn();
    await submit();

    const challenge = { id: '6f1c2a7e-3b4d-4c5e-8f90-1a2b3c4d5e6f', difficulty: 17 };
    server
      .expectOne('/api/sign-up')
      .flush(
        { error: 'challenge_required', challenge },
        { status: 428, statusText: 'Precondition Required' },
      );
    await settle();

    expect(solved).toEqual([challenge]);
    const again = server.expectOne('/api/sign-up');
    expect(again.request.body.challenge).toEqual({ id: challenge.id, nonce: 7 });
    expect(page.querySelector('[role=status]')!.textContent).toContain(
      "Checking that you're not an automated program…",
    );
    again.flush(null, { status: 202, statusText: 'Accepted' });
    await settle();

    expect(page.querySelector('h1')!.textContent).toBe('Check your email');
  });

  it('asks to reload when the terms have changed', async () => {
    await fillIn();
    await submit();

    server
      .expectOne('/api/sign-up')
      .flush({ error: 'terms_changed' }, { status: 409, statusText: 'Conflict' });
    await settle();

    expect(page.querySelector('[role=alert]')!.textContent).toContain(
      'The Terms of Service or the Privacy Notice have changed.',
    );
  });

  it('says when a busy network can sign up again', async () => {
    await fillIn();
    await submit();

    server.expectOne('/api/sign-up').flush(
      { error: 'too_many_requests' },
      {
        status: 429,
        statusText: 'Too Many Requests',
        headers: new HttpHeaders({ 'Retry-After': '600' }),
      },
    );
    await settle();

    const alert = page.querySelector('[role=alert]')!.textContent!;
    expect(alert).toContain('Too many sign-ups have come from your network.');
    expect(alert).toMatch(/Try again after .+\./);
  });

  it('describes other errors without server details', async () => {
    await fillIn();
    await submit();

    server
      .expectOne('/api/sign-up')
      .flush({ error: 'internal' }, { status: 500, statusText: 'Internal Server Error' });
    await settle();

    expect(page.querySelector('[role=alert]')!.textContent).toContain('Something went wrong.');
    expect(page.querySelector('h1')!.textContent).toBe('Sign up');
  });
});
