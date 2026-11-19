# Separate from the default build. Supply the locally verified Python reference image.
ARG PYTHON_REFERENCE=dockervault-r14-python-ref:39124ea
ARG ROLLBACK_PREFLIGHT=dockervault:r14-rollback-preflight
FROM ${ROLLBACK_PREFLIGHT} AS preflight
FROM ${PYTHON_REFERENCE} AS python-rollback
RUN test "$COMMIT_SHA" = 39124ea3a98963b6307a69a0fbaf5902a17cde3d && test -x /usr/bin/flock
LABEL io.dockervault.cutover.role="fenced-python-rollback"
COPY --from=preflight /dockervault-rollback-preflight /usr/local/sbin/
# Check the journal under the native writer fence before exec of legacy init.
ENTRYPOINT ["/usr/bin/flock", "--exclusive", "--nonblock", "--conflict-exit-code", "73", "--no-fork", "/app/data", "/usr/local/sbin/dockervault-rollback-preflight"]
