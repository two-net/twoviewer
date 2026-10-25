import { Routes } from '@angular/router';
import { Offline } from './errors/offline';
import { Unavailable } from './errors/unavailable';
import { AppLayout } from './layout/app-layout';
import { PlainLayout } from './layout/plain-layout';

/**
 * The screens of §4.1 of the SRS. Each screen's code is downloaded when it is first opened,
 * except the Offline and Unavailable pages: they are shown when that download fails, so they
 * must already be there.
 */
export const routes: Routes = [
  { path: '', pathMatch: 'full', redirectTo: 'devices' },
  {
    path: '',
    component: AppLayout,
    children: [
      {
        path: 'devices',
        title: $localize`Devices`,
        loadComponent: () => import('./devices/devices').then((m) => m.Devices),
      },
      {
        path: 'devices/add',
        title: $localize`Add a device`,
        loadComponent: () => import('./devices/add-device').then((m) => m.AddDevice),
      },
      {
        path: 'pair',
        title: $localize`Pair a device`,
        loadComponent: () => import('./devices/pairing').then((m) => m.Pairing),
      },
      {
        path: 'connect',
        title: $localize`Connect`,
        loadComponent: () => import('./sessions/connect').then((m) => m.Connect),
      },
      {
        path: 'vpn',
        title: $localize`VPN`,
        loadComponent: () => import('./vpn/vpn').then((m) => m.Vpn),
      },
      {
        path: 'audit',
        title: $localize`Audit log`,
        loadComponent: () => import('./audit/audit-log').then((m) => m.AuditLog),
      },
      {
        path: 'settings',
        title: $localize`Settings`,
        loadComponent: () => import('./settings/settings').then((m) => m.Settings),
      },
      {
        path: 'operator',
        title: $localize`Operator console`,
        loadComponent: () => import('./operations/operator-console').then((m) => m.OperatorConsole),
      },
    ],
  },
  // The whole window, without the app's header.
  {
    path: 'sessions/:id',
    title: $localize`Remote session`,
    loadComponent: () => import('./sessions/remote-session').then((m) => m.RemoteSession),
  },
  {
    path: '',
    component: PlainLayout,
    children: [
      {
        path: 'sign-in',
        title: $localize`Sign in`,
        loadComponent: () => import('./accounts/sign-in').then((m) => m.SignIn),
      },
      {
        path: 'sign-up',
        title: $localize`Sign up`,
        loadComponent: () => import('./accounts/sign-up').then((m) => m.SignUp),
      },
      {
        path: 'verify-email',
        title: $localize`Verify your email address`,
        loadComponent: () => import('./accounts/verify-email').then((m) => m.VerifyEmail),
      },
      {
        path: 'reset-password',
        title: $localize`Reset your password`,
        loadComponent: () => import('./accounts/reset-password').then((m) => m.ResetPassword),
      },
      {
        path: 'host',
        title: $localize`Remote support`,
        loadComponent: () => import('./sessions/host-page').then((m) => m.HostPage),
      },
      {
        path: 'terms',
        title: $localize`Terms of Service`,
        loadComponent: () => import('./legal/terms').then((m) => m.Terms),
      },
      {
        path: 'privacy',
        title: $localize`Privacy Notice`,
        loadComponent: () => import('./legal/privacy').then((m) => m.Privacy),
      },
      { path: 'offline', title: $localize`You're offline`, component: Offline },
      {
        path: 'unavailable',
        title: $localize`This page couldn't be loaded`,
        component: Unavailable,
      },
      {
        path: '**',
        title: $localize`Page not found`,
        loadComponent: () => import('./errors/not-found').then((m) => m.NotFound),
      },
    ],
  },
];
