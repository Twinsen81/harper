<script lang="ts">
// PROTOTYPE: settings of the local LLM check (an OpenAI-compatible server such as LM Studio).
import { Button, Checkbox, CheckIcon, Input, SettingRow } from 'components';
import { onMount } from 'svelte';
import { Client, type LlmSettings } from '$lib/client';

let settings: LlmSettings | null = null;
let endpointInput = '';
let modelInput = '';
let idleMsInput = '';
let isLoading = true;
let isSaving = false;
let isTesting = false;
let error = '';
let testResult = '';

onMount(() => {
	void load();

	const refresh = () => {
		if (!isSaving) {
			void load();
		}
	};

	window.addEventListener('focus', refresh);

	return () => {
		window.removeEventListener('focus', refresh);
	};
});

function showInInputs(value: LlmSettings) {
	endpointInput = value.endpoint;
	modelInput = value.model;
	idleMsInput = String(value.idleMs);
}

async function load() {
	isLoading = true;
	error = '';

	try {
		settings = await Client.getLlmSettings();
		showInInputs(settings);
	} catch (loadError) {
		error = `Unable to load the AI check settings: ${loadError}`;
	} finally {
		isLoading = false;
	}
}

/** The settings with the values of the text inputs, or `null` if an input is not valid. */
function settingsFromInputs(): LlmSettings | null {
	if (!settings) {
		return null;
	}

	const idleMs = Number(idleMsInput);
	if (!Number.isInteger(idleMs) || idleMs < 0) {
		error = 'The wait time must be a non-negative whole number.';
		return null;
	}

	const endpoint = endpointInput.trim();
	if (!/^https?:\/\//.test(endpoint)) {
		error = 'The endpoint must be an http:// or https:// URL.';
		return null;
	}

	return { ...settings, endpoint, model: modelInput.trim(), idleMs };
}

async function save(next: LlmSettings | null) {
	if (!next || !settings) {
		if (settings) {
			showInInputs(settings);
		}
		return;
	}

	const previous = settings;
	settings = next;
	showInInputs(next);
	isSaving = true;
	error = '';

	try {
		await Client.setLlmSettings(next);
	} catch (saveError) {
		settings = previous;
		showInInputs(previous);
		error = `Unable to save the AI check settings: ${saveError}`;
	} finally {
		isSaving = false;
	}
}

function toggleEnabled() {
	const next = settingsFromInputs();
	void save(next ? { ...next, enabled: !next.enabled } : null);
}

async function testConnection() {
	const next = settingsFromInputs();
	if (!next) {
		return;
	}

	isTesting = true;
	error = '';
	testResult = 'Sending a test sentence...';

	try {
		testResult = await Client.testLlmConnection(next);
	} catch (testError) {
		testResult = `The test failed: ${testError}`;
	} finally {
		isTesting = false;
	}
}
</script>

<div class="stanza">
  <div class="eyebrow">AI check (prototype)</div>
  <p class="section-copy">
    Sends sentences to a language model on this computer, for example in LM Studio, to find
    errors that Harper's rules cannot find, such as a wrong tense. The text does not leave this
    computer if the endpoint is local.
  </p>
  <div class="rows">
    <SettingRow top>
      <strong>Check with a local language model</strong>
      <p>Results show after you stop typing, in addition to Harper's own results.</p>
      <Checkbox
        slot="control"
        appearance="settings"
        checked={settings?.enabled ?? false}
        disabled={isLoading || isSaving || !settings}
        on:click={toggleEnabled}
      >
        {#if settings?.enabled}<CheckIcon className="control-icon" />{/if}
      </Checkbox>
    </SettingRow>
  </div>
  <div class="inline-row">
    <label for="llm-endpoint">Endpoint:</label>
    <Input
      unstyled
      id="llm-endpoint"
      class="select wide"
      type="url"
      disabled={isLoading || isSaving}
      bind:value={endpointInput}
      on:change={() => save(settingsFromInputs())}
    />
  </div>
  <div class="inline-row">
    <label for="llm-model">Model:</label>
    <Input
      unstyled
      id="llm-model"
      class="select wide"
      type="text"
      placeholder="The model that the server has loaded"
      disabled={isLoading || isSaving}
      bind:value={modelInput}
      on:change={() => save(settingsFromInputs())}
    />
  </div>
  <div class="inline-row">
    <label for="llm-idle-ms">Wait after typing:</label>
    <Input
      unstyled
      id="llm-idle-ms"
      class="select"
      type="number"
      min="0"
      step="100"
      disabled={isLoading || isSaving}
      bind:value={idleMsInput}
      on:change={() => save(settingsFromInputs())}
    />
    <span>ms</span>
  </div>
  <div class="inline-row">
    <Button
      unstyled
      class="button"
      type="button"
      disabled={isLoading || isTesting || !settings}
      on:click={testConnection}
    >
      {isTesting ? 'Testing...' : 'Test the connection'}
    </Button>
  </div>
  {#if isLoading}
    <p class="result-summary">Loading the AI check settings...</p>
  {:else if error}
    <p class="result-summary">{error}</p>
  {:else if isSaving}
    <p class="result-summary">Saving the AI check settings...</p>
  {/if}
  {#if testResult}
    <p class="result-summary">{testResult}</p>
  {/if}
</div>
