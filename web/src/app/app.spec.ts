import { Location } from '@angular/common';
import { provideLocationMocks } from '@angular/common/testing';
import { TestBed } from '@angular/core/testing';
import { Title } from '@angular/platform-browser';
import { Router, TitleStrategy, provideRouter } from '@angular/router';
import { App } from './app';
import { routes } from './app.routes';
import { PageTitle } from './layout/page-title';
import { provideFakeSockets } from './shared/connection/testing';

describe('App', () => {
  beforeEach(() => {
    TestBed.configureTestingModule({
      imports: [App],
      providers: [
        provideRouter(routes),
        provideLocationMocks(),
        { provide: TitleStrategy, useClass: PageTitle },
        provideFakeSockets(),
      ],
    });
  });

  async function open(url: string): Promise<HTMLElement> {
    const fixture = TestBed.createComponent(App);
    await TestBed.inject(Router).navigateByUrl(url);
    await fixture.whenStable();
    return fixture.nativeElement;
  }

  const pages = [
    '/devices',
    '/devices/add',
    '/pair',
    '/connect',
    '/vpn',
    '/audit',
    '/settings',
    '/operator',
    '/sessions/1',
    '/sign-in',
    '/sign-up',
    '/verify-email',
    '/reset-password',
    '/host',
    '/terms',
    '/privacy',
    '/offline',
    '/unavailable',
    '/no/such/page',
  ];

  it.each(pages)('shows %s with a heading and the policy links', async (url) => {
    const page = await open(url);

    expect(page.querySelector('main#main h1')?.textContent?.trim()).toBeTruthy();
    const links = [...page.querySelectorAll('footer a')].map((link) => [
      link.textContent?.trim(),
      link.getAttribute('href'),
    ]);
    expect(links).toEqual([
      ['Terms of Service', '/terms'],
      ['Privacy Notice', '/privacy'],
    ]);
  });

  // The remote session view fills the window, without a header.
  it.each(pages.filter((url) => !url.startsWith('/sessions/')))(
    'shows the logo and product name at the top of %s (UI-01)',
    async (url) => {
      const page = await open(url);

      const brand = page.querySelector<HTMLAnchorElement>('header a.brand')!;
      expect(brand.getAttribute('href')).toBe('/');
      expect(brand.textContent?.trim()).toBe('TwoViewer');
      const logo = brand.querySelector('img')!;
      expect(logo.getAttribute('src')).toBe('logo.svg');
      // The name follows the logo, so the logo has no text alternative of its own.
      expect(logo.getAttribute('alt')).toBe('');
    },
  );

  it('opens on the devices, the home screen', async () => {
    const page = await open('/');

    expect(TestBed.inject(Location).path()).toBe('/devices');
    expect(page.querySelector('h1')?.textContent).toBe('Devices');
    expect(page.textContent).toContain('No devices yet. Add your first device.');
  });

  it('shows Page not found for unknown addresses', async () => {
    const page = await open('/no/such/page');

    expect(page.querySelector('h1')?.textContent).toBe('Page not found');
    expect(TestBed.inject(Location).path()).toBe('/no/such/page');
  });

  it('names the browser tab after the page', async () => {
    await open('/audit');

    expect(TestBed.inject(Title).getTitle()).toBe('Audit log – TwoViewer');
  });

  it('skips to the main content without leaving the page', async () => {
    const page = await open('/devices');
    const skipLink = page.querySelector<HTMLAnchorElement>('.skip-link')!;

    skipLink.click();

    expect(document.activeElement?.id).toBe('main');
    expect(TestBed.inject(Location).path()).toBe('/devices');
  });
});
