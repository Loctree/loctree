// F2b fixture: nothing in this fixture imports this module at all, so
// every member of the default-exported object is genuinely dead.
export default {
  findByRunId(runId) {
    return runId;
  },
  getJob() {
    return null;
  },
};
