export interface Plan {
  name: string;
  monthly: number;
  yearly: number;
  perSeat: boolean;
  minSeats: number;
}

export const PLANS: Plan[] = [
  { name: "Personal", monthly: 3, yearly: 30, perSeat: false, minSeats: 1 },
  { name: "Team", monthly: 4, yearly: 40, perSeat: true, minSeats: 3 },
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
