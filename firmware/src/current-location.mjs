// A destination is a user choice. A current position must come from a live,
// successful device state; history and an in-flight request are not evidence.
export function currentBoxPosition(state) {
  if (!state?.online || !state.paired || state.operation || state.mode !== 'simulating') return null;
  const { latitude, longitude } = state.location || {};
  if (typeof latitude !== 'number' || !Number.isFinite(latitude) || latitude < -90 || latitude > 90 ||
      typeof longitude !== 'number' || !Number.isFinite(longitude) || longitude < -180 || longitude > 180) return null;
  return { latitude, longitude };
}

export function positionKey(point) {
  return point ? `${point.latitude},${point.longitude}` : '';
}
