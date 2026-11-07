import { HttpHeaders, provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { Router, provideRouter } from '@angular/router';
import { SignIn } from './sign-in';

describe('SignIn', () => {
  let fixture: ComponentFixture<SignIn>;
  let page: HTMLElement;
  let server: HttpTestingController;
  let navigated: string[];

  beforeEach(async () => {
    TestBed.configureTestingModule({
      imports: [SignIn],
      providers: [provideRouter([]), provideHttpClient(), provideHttpClientTesting()],
    });
    navigated = [];
    vi.spyOn(TestBed.inject(Router), 'navigateByUrl').mockImplementation(async (url) => {
      navigated.push(String(url));
      return true;
    });
    fixture = TestBed.createComponent(SignIn);
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
    await type('sign-in-email', 'ann@example.com');
    await type('sign-in-password', 'violet otter harbor 92');
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

  function alert(): string {
    return page.querySelector('[role=alert]')?.textContent ?? '';
  }

  it('signs in, then goes to the devices', async () => {
    await fillIn();
    await submit();

    const request = server.expectOne('/api/sign-in');
    expect(request.request.method).toBe('POST');
    expect(request.request.body).toEqual({
      email: 'ann@example.com',
      password: 'violet otter harbor 92',
    });
    expect(page.querySelector('[role=status]')!.textContent).toContain('Signing in…');
    request.flush(null, { status: 204, statusText: 'No Content' });
    await settle();

    expect(navigated).toEqual(['/devices']);
  });

  it('helps password managers fill in the form', () => {
    expect(input('sign-in-email').getAttribute('autocomplete')).toBe('username');
    expect(input('sign-in-password').getAttribute('autocomplete')).toBe('current-password');
  });

  it('checks the fields before sending', async () => {
    await type('sign-in-email', 'not an address');
    await submit();

    server.expectNone('/api/sign-in');
    expect(errors()).toEqual([
      'Enter an email address such as name@example.com.',
      'Enter your password.',
    ]);
    await settle();
    expect(document.activeElement).toBe(input('sign-in-email'));
  });

  it('says that the address or password is wrong, without saying which', async () => {
    await fillIn();
    await submit();

    server
      .expectOne('/api/sign-in')
      .flush({ error: 'sign_in_failed' }, { status: 401, statusText: 'Unauthorized' });
    await settle();

    expect(alert()).toContain("The email address or password isn't right.");
    expect(errors()).toEqual([]);
    expect(navigated).toEqual([]);
    expect(page.querySelector<HTMLAnchorElement>('a[href="/reset-password"]')).not.toBeNull();
  });

  it("shows the server's reasons next to the fields, until they change", async () => {
    await fillIn();
    await submit();

    server
      .expectOne('/api/sign-in')
      .flush(
        { error: 'invalid', fields: { email: { code: 'invalid' } } },
        { status: 422, statusText: 'Unprocessable Entity' },
      );
    await settle();

    expect(errors()).toEqual(['Enter an email address such as name@example.com.']);
    expect(document.activeElement).toBe(input('sign-in-email'));
    await type('sign-in-email', 'ann@example.org');
    expect(errors()).toEqual([]);
  });

  it('says when sign-in is possible again after too many attempts', async () => {
    await fillIn();
    await submit();

    server.expectOne('/api/sign-in').flush(
      { error: 'too_many_requests' },
      {
        status: 429,
        statusText: 'Too Many Requests',
        headers: new HttpHeaders({ 'Retry-After': '900' }),
      },
    );
    await settle();

    expect(alert()).toContain('There have been too many attempts to sign in.');
    expect(alert()).toMatch(/Try again after .+\./);
  });

  it('describes other errors without server details', async () => {
    await fillIn();
    await submit();

    server
      .expectOne('/api/sign-in')
      .flush({ error: 'internal' }, { status: 500, statusText: 'Internal Server Error' });
    await settle();

    expect(alert()).toContain('Something went wrong.');
    expect(navigated).toEqual([]);
  });

  describe('with MFA on', () => {
    /** Signs in with the right password, which the server answers with the second step. */
    async function passwordStep(): Promise<void> {
      await fillIn();
      await submit();
      server
        .expectOne('/api/sign-in')
        .flush(
          { error: 'second_factor_required' },
          { status: 428, statusText: 'Precondition Required' },
        );
      await settle();
    }

    async function submitCode(code: string): Promise<void> {
      await type('sign-in-code', code);
      await submit();
    }

    function button(label: string): HTMLButtonElement {
      const found = [...page.querySelectorAll('button')].find(
        (button) => button.textContent!.trim() === label,
      );
      if (!found) {
        throw new Error(`no button "${label}"`);
      }
      return found;
    }

    it('asks for a code from the app, then goes to the devices', async () => {
      await passwordStep();

      expect(navigated).toEqual([]);
      expect(input('sign-in-password')).toBeNull();
      expect(page.textContent).toContain('Enter the code that your authenticator app shows.');
      expect(document.activeElement).toBe(input('sign-in-code'));
      // Phones fill in codes from text messages and apps, with a keypad.
      expect(input('sign-in-code').getAttribute('autocomplete')).toBe('one-time-code');
      expect(input('sign-in-code').getAttribute('inputmode')).toBe('numeric');

      await submitCode('123456');
      const request = server.expectOne('/api/sign-in/second-factor');
      expect(request.request.method).toBe('POST');
      expect(request.request.body).toEqual({ code: '123456' });
      expect(page.querySelector('[role=status]')!.textContent).toContain('Checking the code…');
      request.flush(null, { status: 204, statusText: 'No Content' });
      await settle();

      expect(navigated).toEqual(['/devices']);
    });

    it('asks for the code before sending', async () => {
      await passwordStep();
      await submit();

      server.expectNone('/api/sign-in/second-factor');
      expect(errors()).toEqual(['Enter the code that your authenticator app shows.']);
    });

    it('says when the code is wrong, and lets the user try again', async () => {
      await passwordStep();
      await submitCode('000000');

      server
        .expectOne('/api/sign-in/second-factor')
        .flush({ error: 'second_factor_failed' }, { status: 401, statusText: 'Unauthorized' });
      await settle();

      expect(alert()).toContain("That code isn't right.");
      expect(input('sign-in-code').value).toBe('');
      expect(document.activeElement).toBe(input('sign-in-code'));
      expect(navigated).toEqual([]);
    });

    it('takes a recovery code instead', async () => {
      await passwordStep();
      button('Use a recovery code instead').click();
      await settle();

      expect(page.querySelector('label[for=sign-in-code]')!.textContent).toContain('Recovery code');
      expect(input('sign-in-code').getAttribute('inputmode')).toBeNull();
      expect(input('sign-in-code').getAttribute('autocomplete')).toBe('off');
      expect(document.activeElement).toBe(input('sign-in-code'));

      await submitCode('7k2m-9xqa-4b0d-ztw1');
      server
        .expectOne('/api/sign-in/second-factor')
        .flush({ error: 'second_factor_failed' }, { status: 401, statusText: 'Unauthorized' });
      await settle();
      expect(alert()).toContain("That recovery code isn't right, or has been used.");

      button('Use a code from your app instead').click();
      await settle();
      expect(input('sign-in-code').getAttribute('autocomplete')).toBe('one-time-code');
      expect(alert()).toBe('');
    });

    it('asks for the password again once the second step has expired', async () => {
      await passwordStep();
      await submitCode('123456');

      server
        .expectOne('/api/sign-in/second-factor')
        .flush({ error: 'sign_in_step_expired' }, { status: 410, statusText: 'Gone' });
      await settle();

      expect(alert()).toContain('Signing in took too long.');
      expect(input('sign-in-email').value).toBe('ann@example.com');
      expect(input('sign-in-password').value).toBe('');
      expect(document.activeElement).toBe(input('sign-in-password'));
    });

    it('says when codes may be tried again after too many wrong ones', async () => {
      await passwordStep();
      await submitCode('000000');

      server.expectOne('/api/sign-in/second-factor').flush(
        { error: 'too_many_requests' },
        {
          status: 429,
          statusText: 'Too Many Requests',
          headers: new HttpHeaders({ 'Retry-After': '600' }),
        },
      );
      await settle();

      expect(alert()).toContain('There have been too many wrong codes.');
      expect(alert()).toMatch(/Try again after .+\./);
    });

    it('starts over with the password', async () => {
      await passwordStep();
      button('Start over').click();
      await settle();

      expect(input('sign-in-code')).toBeNull();
      expect(input('sign-in-password').value).toBe('');
      expect(document.activeElement).toBe(input('sign-in-password'));
    });
  });
});
