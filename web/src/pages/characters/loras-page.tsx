import { useEffect, useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { subscribeEvents } from "../../lib/events";
import { useCharacters } from "../../lib/characters";
import { familyLabel, useLoras } from "../../lib/loras";
import { bytes, money } from "../../lib/format";
import type { Job } from "../../lib/types";
import { Link } from "../../lib/router";
import { Icon } from "../../ui/icon";
import { TrainingDialog } from "./training";

export default function LorasPage() {
  const { data, error } = useLoras();
  const characters = useCharacters().data ?? [];
  const [training, setTraining] = useState(false);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    void api.trainingJobs().then(({ jobs }) => { if (live) setJobs(jobs); }).catch((e) => { if (live) setFailure(errorMessage(e)); });
    const stop = subscribeEvents({ job: (job) => { if (job.output === "lora") setJobs((list) => [job, ...list.filter((j) => j.id !== job.id)]); } });
    return () => { live = false; stop(); };
  }, []);
  const action = async (job: Job, cancel: boolean) => { try { if (cancel) await api.cancelJob(job.id); else await api.retryJob(job.id); } catch (e) { setFailure(errorMessage(e)); } };
  return <div className="workspace">
    <div className="intro characters-intro"><div><h1>LoRAs.</h1><p>Trained characters, products and styles for your generations.</p></div><button className="primary-button" type="button" onClick={() => setTraining(true)}><Icon name="add" />Train a LoRA</button></div>
    <p className="control-help"><Link to="/characters">Characters</Link> keep the source images. Choose a compatible LoRA in Create.</p>
    {(error || failure) && <p className="notice error" role="alert">{error || failure}</p>}
    {data.length ? <ul className="lora-list" aria-label="Trained LoRAs">{data.map((lora) => <li key={lora.asset.id} className="lora-row"><Icon name="tune" /><div><h2>{characters.find((c) => c.id === lora.character_id)?.name ?? lora.trigger_word}</h2><p>{familyLabel(lora.family)} / {bytes(lora.asset.size)}</p><p>Trigger: <code>{lora.trigger_word}</code></p></div><a className="text-button" href={lora.asset.url} download={`${lora.trigger_word}.safetensors`}>Download weights<Icon name="download" /></a></li>)}</ul> : <div className="empty"><Icon name="tune" /><h2>No trained LoRAs yet.</h2><p>Choose a saved character or library images to start training.</p></div>}
    {jobs.length > 0 && <section className="training-history"><h2>Training jobs</h2><ul className="lora-list">{jobs.map((job) => <li className="lora-row" key={job.id}><div><h3>{String((job.params._training as unknown as { trigger_word?: string })?.trigger_word ?? job.model_name)}</h3><p>{job.model_name}</p><p role="status">{job.error?.message ?? job.message ?? job.status}{job.cost_usd !== null && ` / ${money(job.cost_usd)}`}</p>{job.status === "running" && job.progress !== null && <progress max={1} value={job.progress} aria-label="Training progress" />}</div>{["queued", "running"].includes(job.status) ? <button type="button" className="text-button" onClick={() => void action(job, true)}>Cancel training</button> : ["failed", "canceled"].includes(job.status) ? <button type="button" className="text-button" onClick={() => void action(job, false)}>Retry training</button> : null}</li>)}</ul></section>}
    {training && <TrainingDialog onClose={() => setTraining(false)} />}
  </div>;
}
