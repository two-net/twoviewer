import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { Router, provideRouter } from '@angular/router';
import { ServerConnection } from '../shared/connection/server-connection';
import { provideFakeSockets } from '../shared/connection/testing';
import { Toasts } from '../shared/toast/toasts';
import { SignOut } from './sign-out';

describe('SignOut', () => {
  let signOut: SignOut;
  let server: HttpTestingController;
  let connection: ServerConnection;
  let toasts: Toasts;
  let navigated: string[];

  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [
        provideRouter([]),
        provideHttpClient(),
        provideHttpClientTesting(),
        provideFakeSockets(),
      ],
    });
    navigated = [];
    vi.spyOn(TestBed.inject(Router), 'navigateByUrl').mockImplementation(async (url) => {
      navigated.push(String(url));
      return true;
    });
    signOut = TestBed.inject(SignOut);
    server = TestBed.inject(HttpTestingController);
    connection = TestBed.inject(ServerConnection);
    toasts = TestBed.inject(Toasts);
    connection.start();
  });

  afterEach(() => server.verify());

  it('ends the sign-in, then goes to the sign-in page', async () => {
    const done = signOut.signOut();

    const request = server.expectOne('/api/sign-out');
    expect(request.request.method).toBe('POST');
    expect(navigated).toEqual([]);
    request.flush(null, { status: 204, statusText: 'No Content' });
    await done;

    expect(navigated).toEqual(['/sign-in']);
    expect(connection.state()).toBe('idle');
    expect(toasts.toasts()).toEqual([]);
  });

  it('stays signed in when the server does not answer', async () => {
    const done = signOut.signOut();

    server
      .expectOne('/api/sign-out')
      .error(new ProgressEvent('error'), { status: 0, statusText: 'Unknown Error' });

    await expect(done).rejects.toBeTruthy();
    expect(navigated).toEqual([]);
    expect(connection.state()).not.toBe('idle');
  });

  it('tells the user when their sign-in has ended elsewhere', async () => {
    await signOut.ended('Signed out.');

    expect(navigated).toEqual(['/sign-in']);
    expect(connection.state()).toBe('idle');
    expect(toasts.toasts().map((toast) => toast.message)).toEqual(['Signed out.']);
  });

  it('does not tell the user who is signing out themselves', async () => {
    const done = signOut.signOut();
    // The server closes the real-time connection before it answers.
    await signOut.ended('Signed out.');
    server.expectOne('/api/sign-out').flush(null, { status: 204, statusText: 'No Content' });
    await done;

    expect(toasts.toasts()).toEqual([]);
  });
});
