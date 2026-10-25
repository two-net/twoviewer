import { Component } from '@angular/core';

/**
 * The remote session view. It fills the window without the app's header, and is designed for
 * windows 1024 px wide or more: the one exception to UI-07.
 */
@Component({
  selector: 'app-remote-session',
  templateUrl: './remote-session.html',
  styleUrl: './remote-session.scss',
})
export class RemoteSession {}
