import { Component } from '@angular/core';
import { RouterLink } from '@angular/router';
import { EmptyState } from '../shared/empty-state/empty-state';

/** The home screen: the user's devices. */
@Component({
  selector: 'app-devices',
  imports: [EmptyState, RouterLink],
  templateUrl: './devices.html',
})
export class Devices {}
