# Frozen future workflow_run context: the event ref is main, but Tests ran on
# develop with the native draft source. This is not a real integration event.
# This test inspects publish inputs and runs only the local source-selection step.
require 'yaml'
require 'open3'
require 'tempfile'

jobs = YAML.safe_load_file(ARGV.fetch(0, '.github/workflows/release.yml')).fetch('jobs')
prepare = jobs.fetch('prepare')
source = prepare.fetch('steps').find { |s| s['id'] == 'source' } or abort 'prepare does not select a shared source'
tested = '683f53bc06b711e0678f69c1273f70e776285be4'
default = 'bec4c670b3d3f91cddefebe8e090d5035cbfc22a'
context = {
  "github.event_name == 'workflow_run' && github.event.workflow_run.head_sha || github.sha" => tested,
  "github.event_name == 'workflow_run' && format('refs/heads/{0}', github.event.workflow_run.head_branch) || github.ref" => 'refs/heads/develop',
  'github.sha' => default,
  'github.ref' => 'refs/heads/main'
}
Tempfile.create('release-outputs') do |output|
  env = source.fetch('env').transform_values do |value|
    context.fetch(value.match(/\A\$\{\{ (.+) \}\}\z/)[1])
  end
  _, error, status = Open3.capture3(env.merge('GITHUB_OUTPUT' => output.path), 'bash', '-euc', source.fetch('run'))
  abort error unless status.success?
  actual = File.readlines(output.path, chomp: true).map { |line| line.split('=', 2) }.to_h
  abort 'develop workflow_run did not select the tested source' unless actual == {
    'sha' => tested, 'ref' => 'refs/heads/develop', 'name' => 'develop'
  }
end

sha = '${{ needs.prepare.outputs.source_sha }}'
[prepare, jobs.fetch('build-and-push'), jobs.fetch('create-release')].each do |job|
  step = job.fetch('steps').find { |s| s.fetch('uses', '').start_with?('actions/checkout@') }
  expected = job.equal?(prepare) ? '${{ steps.source.outputs.sha }}' : sha
  abort 'checkout is not bound to tested SHA' unless step.fetch('with').fetch('ref') == expected
end
meta = prepare.fetch('steps').find { |s| s['id'] == 'docker_meta' }.fetch('with')
abort 'Docker metadata does not use the checked-out source' unless meta.fetch('context') == 'git'
develop_tags = meta.fetch('tags').lines.filter_map do |line|
  case line.chomp
  when /\Atype=raw,value=(.+),enable=\$\{\{ steps\.source\.outputs\.ref == '([^']+)' \}\}\z/
    Regexp.last_match(1) if Regexp.last_match(2) == 'refs/heads/develop'
  when /\Atype=semver,.+,enable=\$\{\{ startsWith\(steps\.source\.outputs\.ref, 'refs\/tags\/v'\) \}\}\z/
    nil
  when 'type=sha,prefix=sha-'
    "sha-#{tested[0, 7]}"
  else
    abort "unrecognized Docker tag rule: #{line}"
  end
end
abort 'develop Docker tags differ' unless develop_tags == ['dev', '${{ steps.version.outputs.version }}', 'sha-683f53b']
abort 'OCI revision is not bound to the source' unless meta.fetch('labels').include?('org.opencontainers.image.revision=${{ steps.source.outputs.sha }}')
build = jobs.fetch('build-and-push').fetch('steps').find { |s| s.fetch('uses', '').start_with?('docker/build-push-action@') }.fetch('with')
abort 'release build must use the tested checkout as its context' unless build.fetch('context') == '.'
abort 'candidate release must build the same Dockerfile default as native CI' if build.key?('target') || build.key?('file')
abort 'release must request SBOM and provenance' unless build['sbom'] == true && build['provenance'] == 'mode=min'
abort 'build args are not bound to the source' unless build.fetch('build-args').include?("COMMIT_SHA=#{sha}\nBRANCH=${{ needs.prepare.outputs.source_name }}")
release = jobs.fetch('create-release').fetch('steps').find { |s| s.fetch('uses', '').start_with?('softprops/action-gh-release@') }.fetch('with')
abort 'release target is not bound to the tested SHA' unless release.fetch('target_commitish') == sha
puts 'PASS: divergent develop workflow_run source, SHA binding and Dockerfile default (no publication)'
