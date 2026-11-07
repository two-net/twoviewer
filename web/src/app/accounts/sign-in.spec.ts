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
});
