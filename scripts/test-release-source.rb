# Frozen workflow_run context: the event ref is main, but Tests ran on develop.
# This test inspects publish inputs and runs only the local source-selection step.
require 'yaml'
require 'open3'
require 'tempfile'

jobs = YAML.safe_load_file(ARGV.fetch(0, '.github/workflows/release.yml')).fetch('jobs')
prepare = jobs.fetch('prepare')
source = prepare.fetch('steps').find { |s| s['id'] == 'source' } or abort 'prepare does not select a shared source'
tested = '39124ea3a98963b6307a69a0fbaf5902a17cde3d'
default = 'c985b370223124f3ce1f8158a060802d3a01c607'
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
abort 'develop Docker tags differ' unless develop_tags == ['dev', '${{ steps.version.outputs.version }}', 'sha-39124ea']
abort 'OCI revision is not bound to the source' unless meta.fetch('labels').include?('org.opencontainers.image.revision=${{ steps.source.outputs.sha }}')
build = jobs.fetch('build-and-push').fetch('steps').find { |s| s.fetch('uses', '').start_with?('docker/build-push-action@') }.fetch('with')
abort 'build args are not bound to the source' unless build.fetch('build-args').include?("COMMIT_SHA=#{sha}\nBRANCH=${{ needs.prepare.outputs.source_name }}")
release = jobs.fetch('create-release').fetch('steps').find { |s| s.fetch('uses', '').start_with?('softprops/action-gh-release@') }.fetch('with')
abort 'release target is not bound to the tested SHA' unless release.fetch('target_commitish') == sha
puts 'PASS: divergent develop workflow_run source and publish inputs (no publication)'
