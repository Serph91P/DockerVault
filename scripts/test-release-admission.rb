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
  const nativeRun = overrides => run({id:43, run_attempt:2, ...overrides});
  const digest = `sha256:${'c'.repeat(64)}`;
  const artifact = (arch, id) => ({id, name:`native-dev-${arch}-${sha}-2`, digest, expired:false});
  const artifacts = [artifact('amd64', 51), artifact('arm64', 52)];
  const matrix = ['amd64', 'arm64'].map(arch => ({name:`Native workspace and offline recovery (${arch})`, head_sha:sha, status:'completed', conclusion:'success'}));
  const execute = new (Object.getPrototypeOf(async function(){}).constructor)('github','context','core','process','setTimeout',script);
  async function trial({ref='refs/heads/develop', tests=run(), native=nativeRun(), jobs=matrix, pending=null,
                       eventName='push', outerRef=ref, outerSha=sha, uploads=artifacts, expectedError=null} = {}) {
    let retries=0, failed=false;
    const outputs={};
    const github={rest:{actions:{listWorkflowRuns:async request => {
      assert.equal(request.head_sha,sha);
      assert.equal(request.event,'push');
      const selected={'test.yml':tests,'rust-native.yml':retries===0 && pending ? pending : native}[request.workflow_id];
      return {data:{workflow_runs:selected ? [selected] : []}};
    },listJobsForWorkflowRun:async request => {
      assert.equal(request.run_id,43);
      return {data:{jobs}};
    },listWorkflowRunArtifacts:Symbol('artifact list')}},request:async (route, request) => {
      assert.equal(route,'GET /repos/{owner}/{repo}/actions/runs/{run_id}/attempts/{attempt_number}/jobs');
      assert.equal(request.run_id,43);
      assert.equal(request.attempt_number,2);
      return {data:{jobs}};
    },paginate:async (method, request) => {
      assert.equal(method,github.rest.actions.listWorkflowRunArtifacts);
      assert.equal(request.run_id,43);
      return uploads.filter(value=>value.name===request.name);
    }};
    const result=execute(github,{repo:{owner:'fixture',repo:'dockervault'},eventName,ref:outerRef,sha:outerSha},
      {setFailed:()=>{failed=true;},info:()=>{},setOutput:(key,value)=>{outputs[key]=value;}},
      {env:{SOURCE_SHA:sha,SOURCE_REF:ref}},callback=>{retries++;callback();});
    if (expectedError) {
      await assert.rejects(result, expectedError);
      assert.equal(outputs.native_run,undefined);
      return false;
    }
    await result;
    if (!failed && ref==='refs/heads/develop') {
      assert.deepEqual(outputs,{native_run:43,native_attempt:2,native_amd64:`51:${digest}`,native_arm64:`52:${digest}`});
    } else assert.deepEqual(outputs,{});
    return !failed;
  }
  (async()=>{
    assert.equal(await trial(),true);
    assert.equal(await trial({ref:'refs/tags/v1.2.3'}),true);
    assert.equal(await trial({ref:'refs/heads/main', tests:run({head_branch:'main'}), native:nativeRun({head_branch:'main'})}),true);
    assert.equal(await trial({pending:nativeRun({status:'in_progress',conclusion:null})}),true);
    for (const settings of [
      {native:nativeRun({conclusion:'failure'})}, {tests:run({conclusion:'failure'})}, {native:null},
      {ref:'refs/tags/v1.2.3',native:nativeRun({event:'pull_request'})},
      {native:nativeRun({head_sha:other})}, {tests:run({head_sha:other})}, {tests:run({head_branch:'main'})},
      {jobs:[matrix[0]]}, {jobs:[matrix[0],{...matrix[1],conclusion:'skipped'}]},
      {jobs:[matrix[0],{...matrix[1],head_sha:other}]}, {jobs:[...matrix,matrix[1]]},
      {native:nativeRun({event:'pull_request'})},
      {eventName:'workflow_run',expectedError:/direct source push/},
      {outerRef:'refs/heads/main',expectedError:/direct source push/},
      {outerSha:other,expectedError:/direct source push/},
      {uploads:[artifacts[0]],expectedError:/unique authenticated artifact/},
      {uploads:[...artifacts,artifacts[0]],expectedError:/unique authenticated artifact/},
      {uploads:[{...artifacts[0],expired:true},artifacts[1]],expectedError:/unique authenticated artifact/},
      {uploads:[{...artifacts[0],digest:'bad'},artifacts[1]],expectedError:/unique authenticated artifact/},
      {uploads:[{...artifacts[0],id:0},artifacts[1]],expectedError:/unique authenticated artifact/},
    ]) assert.equal(await trial(settings),false,JSON.stringify(settings));
    console.log('PASS: exact-SHA release admission, both native architectures, no publication');
  })().catch(error=>{console.error(error);process.exitCode=1;});
JS
out, error, status = Open3.capture3('node', '-e', fixture, stdin_data: JSON.generate(gate.fetch('with').fetch('script')))
abort error unless status.success?
puts out
