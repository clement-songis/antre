const M = (window.__antre_modules || {})["stores/session-store"] || {};
export default M;
export const sessionStore = M.sessionStore;
export const insertSessionInOrder = (...args) => M.insertSessionInOrder?.(...args);
