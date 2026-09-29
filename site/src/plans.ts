export interface Plan {
  name: string;
  monthly: number;
  yearly: number;
  perSeat: boolean;
  minSeats: number;
  sitesPerSeat: number;
}

export const TRIAL_DAYS = 30;

export const PLANS: Plan[] = [
  { name: "Personal", monthly: 3, yearly: 30, perSeat: false, minSeats: 1, sitesPerSeat: 3 },
  { name: "Team", monthly: 4, yearly: 40, perSeat: true, minSeats: 3, sitesPerSeat: 10 },
];

export function euros(amount: number): string {
  return `€${amount}`;
}

export function monthsFree(plan: Plan): number {
  return Math.floor((plan.monthly * 12 - plan.yearly) / plan.monthly);
}

export function lowest(plans: readonly Plan[]): number {
  return Math.min(...plans.map((plan) => plan.monthly));
}

export function people(plan: Plan): string {
  return plan.perSeat ? `${plan.minSeats} or more` : `${plan.minSeats}`;
}

export function sites(plan: Plan): string {
  return plan.perSeat ? `${plan.sitesPerSeat} per seat` : `${plan.sitesPerSeat}`;
}
