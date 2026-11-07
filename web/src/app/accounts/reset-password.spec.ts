import { Location } from '@angular/common';
import { HttpHeaders, provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { provideLocationMocks } from '@angular/common/testing';
import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { ResetPassword } from './reset-password';

describe('ResetPassword', () => {
  let server: HttpTestingController;
  let harness: RouterTestingHarness;
  let page: HTMLElement;

  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'reset-password', component: ResetPassword }]),
        provideLocationMocks(),
        provideHttpClient(),
        provideHttpClientTesting(),
      ],
    });
    server = TestBed.inject(HttpTestingController);
  });

  afterEach(() => server.verify());

  async function open(url: string): Promise<void> {
    harness = await RouterTestingHarness.create(url);
    page = harness.routeNativeElement!;
  }

  /** Lets the component handle a response, which takes a few turns of promises. */
  async function settle(): Promise<void> {
    for (let i = 0; i < 5; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      await harness.fixture.whenStable();
    }
  }

  function input(id: string): HTMLInputElement {
    return page.querySelector<HTMLInputElement>(`#${id}`)!;
  }

  async function type(id: string, value: string): Promise<void> {
    input(id).value = value;
    input(id).dispatchEvent(new Event('input'));
    await harness.fixture.whenStable();
  }

  async function submit(): Promise<void> {
    page.querySelector<HTMLButtonElement>('button[type=submit]')!.click();
    await harness.fixture.whenStable();
  }

  function button(label: string): HTMLButtonElement {
    return Array.from(page.querySelectorAll('button')).find(
      (button) => button.textContent!.trim() === label,
    )!;
  }

  function heading(): string {
    return page.querySelector('h1')!.textContent!.trim();
  }

  function errors(): string[] {
    return Array.from(page.querySelectorAll('.field-error'), (error) => error.textContent!.trim());
  }

  function alert(): string {
    return page.querySelector('[role=alert]')?.textContent ?? '';
  }

  /** Opens the page with a link whose token works, for ann@example.com. */
  async function openLink(): Promise<void> {
    await open('/reset-password#0123abcd');
    server
      .expectOne('/api/password-reset/check')
      .flush({ email: 'ann@example.com' }, { status: 200, statusText: 'OK' });
    await settle();
  }

  describe('without a link', () => {
    beforeEach(async () => {
      await open('/reset-password');
    });

    it('asks for a link, then says to check the email', async () => {
      expect(heading()).toBe('Reset your password');
      await type('reset-password-email', ' Ann@Example.com ');
      await submit();

      const request = server.expectOne('/api/password-reset');
      expect(request.request.method).toBe('POST');
      expect(request.request.body).toEqual({ email: 'Ann@Example.com' });
      request.flush(null, { status: 202, statusText: 'Accepted' });
      await settle();

      expect(heading()).toBe('Check your email');
      expect(document.activeElement).toBe(page.querySelector('h1'));
      expect(page.querySelector('.address')!.textContent).toBe('Ann@Example.com');
      expect(page.textContent).toContain('The link works for 1 hour.');

      // Another link, for example when the first went to the spam folder.
      button('Send another link').click();
      await settle();
      expect(input('reset-password-email').value).toBe('Ann@Example.com');
      expect(document.activeElement).toBe(input('reset-password-email'));
    });

    it('checks the address before sending', async () => {
      await submit();

      server.expectNone('/api/password-reset');
      expect(errors()).toEqual(['Enter your email address.']);

      await type('reset-password-email', 'not an address');
      expect(errors()).toEqual(['Enter an email address such as name@example.com.']);
    });

    it("shows the server's reason next to the field", async () => {
      await type('reset-password-email', 'ann@example');
      await submit();

      server
        .expectOne('/api/password-reset')
        .flush(
          { error: 'invalid', fields: { email: { code: 'invalid' } } },
          { status: 422, statusText: 'Unprocessable Entity' },
        );
      await settle();

      expect(errors()).toEqual(['Enter an email address such as name@example.com.']);
      expect(document.activeElement).toBe(input('reset-password-email'));
    });

    it('says when a busy network may ask again', async () => {
      await type('reset-password-email', 'ann@example.com');
      await submit();

      server.expectOne('/api/password-reset').flush(
        { error: 'too_many_requests' },
        {
          status: 429,
          statusText: 'Too Many Requests',
          headers: new HttpHeaders({ 'Retry-After': '900' }),
        },
      );
      await settle();

      expect(alert()).toContain('Too many password resets have been asked for from your network.');
      expect(alert()).toMatch(/Try again after .+\./);
      expect(heading()).toBe('Reset your password');
    });
  });

  describe('with a link', () => {
    it('checks the token from the fragment, and takes it out of the address bar', async () => {
      await open('/reset-password#0123abcd');

      const request = server.expectOne('/api/password-reset/check');
      expect(request.request.method).toBe('POST');
      expect(request.request.body).toEqual({ token: '0123abcd' });
      expect(TestBed.inject(Location).path(true)).toBe('/reset-password');
      expect(page.textContent).toContain('Checking your link…');

      request.flush({ email: 'ann@example.com' }, { status: 200, statusText: 'OK' });
      await settle();

      expect(heading()).toBe('Choose a new password');
      expect(page.querySelector('.address')!.textContent).toBe('ann@example.com');
      // For password managers, which save the new password under this address.
      const username = page.querySelector<HTMLInputElement>('input[autocomplete=username]')!;
      expect(username.value).toBe('ann@example.com');
      expect(username.hidden).toBe(true);
      expect(input('reset-password-new').autocomplete).toBe('new-password');
      expect(document.activeElement).toBe(input('reset-password-new'));
    });

    it('sets the new password, then offers to sign in', async () => {
      await openLink();
      await type('reset-password-new', 'amber lantern meadow 41');
      await submit();

      const request = server.expectOne('/api/password-reset/complete');
      expect(request.request.body).toEqual({
        token: '0123abcd',
        password: 'amber lantern meadow 41',
      });
      request.flush(null, { status: 204, statusText: 'No Content' });
      await settle();

      expect(heading()).toBe('Your password is reset');
      expect(document.activeElement).toBe(page.querySelector('h1'));
      expect(page.textContent).toContain("You've been signed out everywhere.");
      expect(page.querySelector('a[href="/sign-in"]')).not.toBeNull();
    });

    it('checks the password before sending', async () => {
      await openLink();
      await type('reset-password-new', 'short');
      await submit();

      server.expectNone('/api/password-reset/complete');
      expect(errors()).toEqual(['Use at least 12 characters.']);
    });

    it("shows the server's reason next to the password", async () => {
      await openLink();
      await type('reset-password-new', 'password1234');
      await submit();

      server
        .expectOne('/api/password-reset/complete')
        .flush(
          { error: 'invalid', fields: { password: { code: 'compromised' } } },
          { status: 422, statusText: 'Unprocessable Entity' },
        );
      await settle();

      expect(errors()).toEqual([
        'This password is on a list of passwords stolen from other sites. Choose another one.',
      ]);
      expect(input('reset-password-new').getAttribute('aria-describedby')).toBe(
        'reset-password-hint reset-password-new-error',
      );

      // Until the password changes.
      await type('reset-password-new', 'amber lantern meadow 41');
      expect(errors()).toEqual([]);
    });

    it('offers a new link when the link has expired', async () => {
      await open('/reset-password#0123abcd');
      server
        .expectOne('/api/password-reset/check')
        .flush({ error: 'link_expired' }, { status: 410, statusText: 'Gone' });
      await settle();

      expect(heading()).toBe("This link doesn't work any more");
      expect(page.textContent).toContain('A link works once, for 1 hour.');

      button('Get a new link').click();
      await settle();
      expect(heading()).toBe('Reset your password');
      expect(document.activeElement).toBe(input('reset-password-email'));
    });

    it('says so when the link was used up in the meantime', async () => {
      await openLink();
      await type('reset-password-new', 'amber lantern meadow 41');
      await submit();

      server
        .expectOne('/api/password-reset/complete')
        .flush({ error: 'link_expired' }, { status: 410, statusText: 'Gone' });
      await settle();

      expect(heading()).toBe("This link doesn't work any more");
    });

    it('lets the user try again when the link cannot be checked', async () => {
      await open('/reset-password#0123abcd');
      server
        .expectOne('/api/password-reset/check')
        .flush(null, { status: 503, statusText: 'Service Unavailable' });
      await settle();

      expect(alert()).toContain("can't be reached right now.");

      button('Try again').click();
      await settle();
      server
        .expectOne('/api/password-reset/check')
        .flush({ email: 'ann@example.com' }, { status: 200, statusText: 'OK' });
      await settle();
      expect(heading()).toBe('Choose a new password');
    });
  });
});
