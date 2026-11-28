const M = (window.__antre_modules || {})["stores/session-history-cache"] || {};
export default M;
export const getSessionHistory = (...args) => M.getSessionHistory?.(...args);
export const setSessionHistory = (...args) => M.setSessionHistory?.(...args);
export const clearSessionHistory = (...args) => M.clearSessionHistory?.(...args);
