import { Location } from '@angular/common';
import { provideLocationMocks } from '@angular/common/testing';
import { TestBed } from '@angular/core/testing';
import { provideRouter, withNavigationErrorHandler } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { showErrorPage } from './navigation-error';
import { Offline } from './offline';
import { RELOAD } from './reload';
import { Unavailable } from './unavailable';

describe('showErrorPage', () => {
  let reload: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    reload = vi.fn();
    vi.spyOn(console, 'error').mockImplementation(() => {});
    TestBed.configureTestingModule({
      providers: [
        provideRouter(
          [
            {
              path: 'screen',
              loadComponent: () =>
                Promise.reject(new TypeError('Failed to fetch dynamically imported module')),
            },
            { path: 'offline', component: Offline },
            { path: 'unavailable', component: Unavailable },
          ],
          withNavigationErrorHandler(showErrorPage),
        ),
        provideLocationMocks(),
        { provide: RELOAD, useValue: reload },
      ],
    });
  });

  afterEach(() => vi.restoreAllMocks());

  async function openScreen(): Promise<HTMLElement> {
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/screen');
    return harness.routeNativeElement!;
  }

  it('shows Unavailable when online, keeping the address', async () => {
    const page = await openScreen();

    expect(page.querySelector('h1')?.textContent).toBe("This page couldn't be loaded");
    expect(TestBed.inject(Location).path()).toBe('/screen');
  });

  it('shows Offline when offline, keeping the address', async () => {
    vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(false);

    const page = await openScreen();

    expect(page.querySelector('h1')?.textContent).toBe("You're offline");
    expect(TestBed.inject(Location).path()).toBe('/screen');
  });

  it('loads the address as a new page on Retry', async () => {
    const page = await openScreen();

    page.querySelector('button')!.click();

    expect(reload).toHaveBeenCalledOnce();
  });

  it('loads the address as a new page when the browser is back online', async () => {
    vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(false);
    await openScreen();
    expect(reload).not.toHaveBeenCalled();

    window.dispatchEvent(new Event('online'));

    expect(reload).toHaveBeenCalledOnce();
  });
});
