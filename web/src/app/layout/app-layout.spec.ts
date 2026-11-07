import { provideHttpClient } from '@angular/common/http';
import { provideHttpClientTesting } from '@angular/common/http/testing';
import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Router, provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { NOT_SIGNED_IN } from '../shared/connection/messages';
import { ServerConnection } from '../shared/connection/server-connection';
import { FakeSockets, provideFakeSockets } from '../shared/connection/testing';
import { Toasts } from '../shared/toast/toasts';
import { AppLayout } from './app-layout';

@Component({ template: '' })
class Screen {}

describe('AppLayout', () => {
  let harness: RouterTestingHarness;
  let page: HTMLElement;
  let menuButton: HTMLButtonElement;
  let sockets: FakeSockets;

  beforeEach(async () => {
    sockets = new FakeSockets();
    TestBed.configureTestingModule({
      providers: [
        provideRouter([
          {
            path: '',
            component: AppLayout,
            children: [
              { path: 'devices', component: Screen },
              { path: 'vpn', component: Screen },
            ],
          },
          { path: 'sign-in', component: Screen },
        ]),
        provideHttpClient(),
        provideHttpClientTesting(),
        provideFakeSockets(sockets),
      ],
    });
    harness = await RouterTestingHarness.create('/devices');
    page = harness.fixture.nativeElement;
    menuButton = page.querySelector('.menu-button')!;
  });

  async function toggleMenu(): Promise<void> {
    menuButton.click();
    await harness.fixture.whenStable();
  }

  it('opens and closes the navigation with the menu button', async () => {
    expect(menuButton.getAttribute('aria-expanded')).toBe('false');

    await toggleMenu();
    expect(menuButton.getAttribute('aria-expanded')).toBe('true');
    expect(page.querySelector('#primary-nav')!.classList).toContain('open');

    await toggleMenu();
    expect(menuButton.getAttribute('aria-expanded')).toBe('false');
  });

  it('closes the navigation after navigating', async () => {
    await toggleMenu();

    await harness.navigateByUrl('/vpn');

    expect(menuButton.getAttribute('aria-expanded')).toBe('false');
  });

  it('closes the navigation on Escape and returns focus to the menu button', async () => {
    await toggleMenu();

    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    await harness.fixture.whenStable();

    expect(menuButton.getAttribute('aria-expanded')).toBe('false');
    expect(document.activeElement).toBe(menuButton);
  });

  it('opens the connection to the server', () => {
    expect(TestBed.inject(ServerConnection).state()).toBe('connecting');
  });

  it('goes to the sign-in page when the sign-in ends, and says why', async () => {
    sockets.last.receive({ type: 'hello', heartbeatTimeoutMs: 8000 });

    sockets.last.drop(NOT_SIGNED_IN);
    await harness.fixture.whenStable();

    expect(TestBed.inject(Router).url).toBe('/sign-in');
    expect(TestBed.inject(ServerConnection).state()).toBe('idle');
    expect(
      TestBed.inject(Toasts)
        .toasts()
        .map((toast) => toast.message),
    ).toEqual(["You've been signed out. Sign in again to continue."]);
  });

  it('goes to the sign-in page without a word when the browser was not signed in', async () => {
    sockets.last.drop(NOT_SIGNED_IN);
    await harness.fixture.whenStable();

    expect(TestBed.inject(Router).url).toBe('/sign-in');
    expect(TestBed.inject(Toasts).toasts()).toEqual([]);
  });

  it('marks the current screen', () => {
    const current = page.querySelector('[aria-current="page"]');

    expect(current?.textContent?.trim()).toBe('Devices');
  });
});
