# Check the real version selector against a small, local Git history.
require 'open3'
require 'tmpdir'
require 'yaml'

root = File.expand_path('..', __dir__)
release = YAML.safe_load_file(File.join(root, '.github/workflows/release.yml'))
native = YAML.safe_load_file(File.join(root, '.github/workflows/rust-native.yml'))
version_step = release.fetch('jobs').fetch('prepare').fetch('steps').find { |s| s['id'] == 'version' }
build_step = native.fetch('jobs').fetch('native-workspace').fetch('steps').find { |s| s['name'].to_s.start_with?('Build the tested OCI') }
abort 'Development OCI does not pass the selected OCI version label to Buildx' unless build_step.fetch('run').include?('--label "org.opencontainers.image.version=$DV_VERSION"')
abort 'Native and Release must run the same version selector' unless build_step.fetch('run').include?('scripts/release-version.sh') && version_step.fetch('run').include?('scripts/release-version.sh')
Dir.mktmpdir('dv-version') do |dir|
  git = ->(*args) do
    out, err, status = Open3.capture3('git', '-C', dir, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', *args)
    abort err unless status.success?
    out.strip
  end
  git.call('init', '-q')
  git.call('commit', '-q', '--allow-empty', '-m', 'release baseline')
  git.call('tag', 'v4.11.1-dev')
  git.call('commit', '-q', '--allow-empty', '-m', 'feat(recovery): fixture')
  sha = git.call('rev-parse', 'HEAD')
  selector = File.join(root, 'scripts/release-version.sh')
  check = ->(ref, version, prerelease, mode) do
    out, err, rc = Open3.capture3('bash', selector, ref, sha, chdir: dir)
    abort "#{ref}: #{out} #{err}" unless rc.success? && out.lines.map(&:chomp) == ["version=#{version}", "is_prerelease=#{prerelease}", "build_env=#{mode}"]
  end
  check.call('refs/heads/develop', '4.12.0-dev', true, 'development')
  check.call('refs/heads/main', '4.12.0', false, 'production')
  check.call('refs/tags/v3.2.1', '3.2.1', false, 'production')
  check.call('refs/tags/v3.2.1-dev', '3.2.1-dev', true, 'development')
  _, _, rc = Open3.capture3('bash', selector, 'refs/heads/develop', 'b' * 40, chdir: dir)
  abort 'version selection accepted a wrong checkout' if rc.success?
end
puts 'PASS: shared version from Git, Develop dev version, source binding and unchanged main/tag behavior'
