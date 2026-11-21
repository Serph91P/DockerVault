# Run the real release gate with local Actions API fixtures. No GitHub or publication calls.
require 'json'
require 'open3'
require 'yaml'

workflow = YAML.safe_load_file(ARGV.fetch(0, '.github/workflows/release.yml'))
jobs = workflow.fetch('jobs')
prepare = jobs.fetch('prepare')
steps = prepare.fetch('steps')
gate = steps.find { |step| step['id'] == 'admission' } or abort 'release preparation has no shared admission gate'
abort 'admission must run after source selection for every eligible event' if gate.key?('if') ||
  gate['continue-on-error'] || steps.index(gate) <= steps.index { |step| step['id'] == 'source' }
abort 'admission is not bound to the selected SHA/ref' unless gate['env'] == {
  'SOURCE_SHA' => '${{ steps.source.outputs.sha }}', 'SOURCE_REF' => '${{ steps.source.outputs.ref }}'
}
abort 'publication must depend on release preparation' unless jobs.fetch('build-and-push')['needs'] == 'prepare' &&
  jobs.fetch('create-release')['needs'] == %w[prepare build-and-push] &&
  [jobs.fetch('build-and-push'), jobs.fetch('create-release')].none? { |job| job.fetch('if', '').include?('always()') }
trigger = workflow['on'] || workflow.fetch(true)
abort 'release event policy changed' unless trigger.fetch('workflow_run').fetch('branches') == %w[main develop] &&
  trigger.fetch('workflow_run').fetch('workflows') == ['Tests'] &&
  trigger.fetch('push').fetch('branches') == %w[main develop] && trigger.fetch('push').fetch('tags') == ['v*'] &&
  prepare.fetch('if').include?("github.event.workflow_run.event == 'push'")

fixture = <<~'JS'
  const assert = require('node:assert/strict');
  const script = JSON.parse(require('node:fs').readFileSync(0, 'utf8'));
  const sha = 'a'.repeat(40), other = 'b'.repeat(40);
  const run = overrides => ({id:42, head_sha:sha, head_branch:'develop', event:'push', status:'completed', conclusion:'success', ...overrides});
  const matrix = ['amd64', 'arm64'].map(arch => ({name:`Native workspace and offline recovery (${arch})`, head_sha:sha, status:'completed', conclusion:'success'}));
  const execute = new (Object.getPrototypeOf(async function(){}).constructor)('github','context','core','process','setTimeout',script);
  async function trial({ref='refs/heads/develop', tests=run(), native=run(), jobs=matrix, pending=null} = {}) {
    let retries=0, failed=false;
    const github={rest:{actions:{listWorkflowRuns:async request => {
      assert.equal(request.head_sha,sha);
      assert.equal(request.event,'push');
      const selected={'test.yml':tests,'rust-native.yml':retries===0 && pending ? pending : native}[request.workflow_id];
      return {data:{workflow_runs:selected ? [selected] : []}};
    },listJobsForWorkflowRun:async request => {
      assert.equal(request.run_id,42);
      return {data:{jobs}};
    }}}};
    await execute(github,{repo:{owner:'fixture',repo:'dockervault'},sha:other},
      {setFailed:()=>{failed=true;},info:()=>{}}, {env:{SOURCE_SHA:sha,SOURCE_REF:ref}}, callback=>{retries++;callback();});
    return !failed;
  }
  (async()=>{
    assert.equal(await trial(),true);
    assert.equal(await trial({ref:'refs/tags/v1.2.3'}),true);
    assert.equal(await trial({pending:run({status:'in_progress',conclusion:null})}),true);
    for (const settings of [
      {native:run({conclusion:'failure'})}, {tests:run({conclusion:'failure'})}, {native:null},
      {ref:'refs/tags/v1.2.3',native:run({event:'pull_request'})},
      {native:run({head_sha:other})}, {tests:run({head_branch:'main'})},
      {jobs:[matrix[0]]}, {jobs:[matrix[0],{...matrix[1],conclusion:'skipped'}]},
      {jobs:[matrix[0],{...matrix[1],head_sha:other}]},
    ]) assert.equal(await trial(settings),false,JSON.stringify(settings));
    console.log('PASS: exact-SHA release admission, both native architectures, no publication');
  })().catch(error=>{console.error(error);process.exitCode=1;});
JS
out, error, status = Open3.capture3('node', '-e', fixture, stdin_data: JSON.generate(gate.fetch('with').fetch('script')))
abort error unless status.success?
puts out
