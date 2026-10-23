// The installed TypeScript SDK is the behavioral oracle for the native model adapters.
import { createOpenAICompatible } from '@ai-sdk/openai-compatible';
import { createOpenAI } from '@ai-sdk/openai';
import { createAnthropic } from '@ai-sdk/anthropic';
let input = '';
for await (const bytes of process.stdin) input += bytes;
const test = JSON.parse(input);
let request;
const fetch = async (url, init) => {
  request = { url: String(url), body: JSON.parse(init.body) };
  return new Response(test.events, { status: 200, headers: { 'content-type': 'text/event-stream' } });
};
const settings = { baseURL: 'http://fixture/v1', apiKey: 'fixture-key', fetch };
const model = test.provider === 'anthropic' ? createAnthropic(settings).messages(test.model)
  : test.provider === 'openai' ? createOpenAI(settings).responses(test.model)
  : createOpenAICompatible({ ...settings, name: test.name ?? 'fixture', includeUsage: true }).chatModel(test.model);
let answer;
try {
  answer = await model.doStream(test.options);
} catch (error) {
  process.stdout.write(JSON.stringify({ request, error: { message: error.message, url: error.url, requestBodyValues: error.requestBodyValues, statusCode: error.statusCode, responseBody: error.responseBody, isRetryable: error.isRetryable } }));
  process.exit(0);
}
const parts = [];
// Rust represents JavaScript Date values as epoch milliseconds.
for await (const part of answer.stream) {
  if (part.type === "source") {
    if (!/^[A-Za-z0-9]{16}$/.test(part.id)) throw new Error("Invalid generated SDK source id");
    parts.push({ ...part, id: "generated-source-id" });
  } else parts.push(part.type === "response-metadata" && part.timestamp instanceof Date ? { ...part, timestamp: part.timestamp.getTime() } : part);
}
process.stdout.write(JSON.stringify({ request, parts }));
