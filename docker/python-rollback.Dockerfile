# Separate from the default build. Supply the locally verified Python reference image.
ARG PYTHON_REFERENCE=dockervault-r14-python-ref:39124ea
FROM ${PYTHON_REFERENCE} AS python-rollback
RUN test "$COMMIT_SHA" = 39124ea3a98963b6307a69a0fbaf5902a17cde3d && test -x /usr/bin/flock
LABEL io.dockervault.cutover.role="fenced-python-rollback"
# Match native-candidate. Keep the original Python entrypoint, application and format.
ENTRYPOINT ["/usr/bin/flock", "--exclusive", "--nonblock", "--conflict-exit-code", "73", "--no-fork", "/app/data", "/entrypoint.sh"]
