import { Location } from '@angular/common';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { provideLocationMocks } from '@angular/common/testing';
import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { VerifyEmail } from './verify-email';

describe('VerifyEmail', () => {
  let server: HttpTestingController;

  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'verify-email', component: VerifyEmail }]),
        provideLocationMocks(),
        provideHttpClient(),
        provideHttpClientTesting(),
      ],
    });
    server = TestBed.inject(HttpTestingController);
  });

  afterEach(() => server.verify());

  async function open(url: string): Promise<{ page: HTMLElement; harness: RouterTestingHarness }> {
    const harness = await RouterTestingHarness.create(url);
    return { page: harness.routeNativeElement!, harness };
  }

  async function settle(harness: RouterTestingHarness): Promise<void> {
    for (let i = 0; i < 3; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      await harness.fixture.whenStable();
    }
  }

  it('sends the token from the fragment, and takes it out of the address bar', async () => {
    const { page, harness } = await open('/verify-email#0123abcd');

    const request = server.expectOne('/api/verify-email');
    expect(request.request.method).toBe('POST');
    expect(request.request.body).toEqual({ token: '0123abcd' });
    expect(TestBed.inject(Location).path(true)).toBe('/verify-email');
    expect(page.textContent).toContain('Verifying your email address…');

    request.flush(null, { status: 204, statusText: 'No Content' });
    await settle(harness);

    expect(page.textContent).toContain('Your email address is verified.');
    expect(page.querySelector('a[href="/sign-in"]')).not.toBeNull();
  });

  it('offers a new link when the link has expired', async () => {
    const { page, harness } = await open('/verify-email#0123abcd');

    server
      .expectOne('/api/verify-email')
      .flush({ error: 'link_expired' }, { status: 410, statusText: 'Gone' });
    await settle(harness);

    expect(page.textContent).toContain("This link has expired or isn't valid.");
    expect(page.querySelector('a[href="/sign-up"]')).not.toBeNull();
  });

  it('explains where the link is when there is no token', async () => {
    const { page } = await open('/verify-email');

    server.expectNone('/api/verify-email');
    expect(page.textContent).toContain('open the link in the email we sent you');
  });

  it('can try again after an error', async () => {
    const { page, harness } = await open('/verify-email#0123abcd');

    server
      .expectOne('/api/verify-email')
      .flush(null, { status: 503, statusText: 'Service Unavailable' });
    await settle(harness);

    expect(page.querySelector('[role=alert]')!.textContent).toContain("can't be reached right now");
    page.querySelector<HTMLButtonElement>('button')!.click();
    await harness.fixture.whenStable();

    const again = server.expectOne('/api/verify-email');
    expect(again.request.body).toEqual({ token: '0123abcd' });
    again.flush(null, { status: 204, statusText: 'No Content' });
    await settle(harness);
    expect(page.textContent).toContain('Your email address is verified.');
  });
});
