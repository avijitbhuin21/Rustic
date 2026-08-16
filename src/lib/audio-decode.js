// Audio decoding for the editor's audio preview.
//
// The native <audio> element is not usable as the primary player: Chromium
// (and therefore WebView2) rejects any stream whose sample rate falls outside
// its supported window — 2 kHz clinical recordings (heart / lung sounds) load
// but report duration 0 and refuse to play. So we decode ourselves: RIFF/WAVE
// is parsed byte-for-byte here, everything else goes through decodeAudioData,
// and the result is resampled to the AudioContext's rate.

const WAVE_FORMAT_PCM = 0x0001;
const WAVE_FORMAT_FLOAT = 0x0003;
const WAVE_FORMAT_ALAW = 0x0006;
const WAVE_FORMAT_MULAW = 0x0007;
const WAVE_FORMAT_EXTENSIBLE = 0xfffe;

function fourcc(view, offset) {
  return String.fromCharCode(
    view.getUint8(offset),
    view.getUint8(offset + 1),
    view.getUint8(offset + 2),
    view.getUint8(offset + 3),
  );
}

/** Expands an 8-bit A-law sample to a signed 16-bit value. */
function alawToPcm(a) {
  let v = a ^ 0x55;
  const sign = v & 0x80;
  const exponent = (v & 0x70) >> 4;
  let mantissa = v & 0x0f;
  let sample = exponent === 0 ? (mantissa << 4) + 8 : ((mantissa << 4) + 0x108) << (exponent - 1);
  return sign ? -sample : sample;
}

/** Expands an 8-bit mu-law sample to a signed 16-bit value. */
function mulawToPcm(u) {
  const v = ~u & 0xff;
  const sign = v & 0x80;
  const exponent = (v & 0x70) >> 4;
  const mantissa = v & 0x0f;
  const sample = (((mantissa << 3) + 0x84) << exponent) - 0x84;
  return sign ? -sample : sample;
}

/**
 * Parses a RIFF/WAVE buffer into planar float channels plus its format.
 */
export function parseWav(arrayBuffer) {
  const view = new DataView(arrayBuffer);
  if (view.byteLength < 12) throw new Error('Not a WAV file');
  const riff = fourcc(view, 0);
  const wave = fourcc(view, 8);
  const bigEndian = riff === 'RIFX';
  if ((riff !== 'RIFF' && riff !== 'RIFX') || wave !== 'WAVE') throw new Error('Not a WAV file');
  const le = !bigEndian;

  let fmt = null;
  let dataOffset = -1;
  let dataLength = 0;

  let pos = 12;
  while (pos + 8 <= view.byteLength) {
    const id = fourcc(view, pos);
    const size = view.getUint32(pos + 4, le);
    const body = pos + 8;
    if (id === 'fmt ') {
      const formatTag = view.getUint16(body, le);
      const channels = view.getUint16(body + 2, le);
      const sampleRate = view.getUint32(body + 4, le);
      const bitsPerSample = view.getUint16(body + 14, le);
      let codec = formatTag;
      if (formatTag === WAVE_FORMAT_EXTENSIBLE && size >= 40) {
        // The real codec is the first two bytes of the SubFormat GUID.
        codec = view.getUint16(body + 24, le);
      }
      fmt = { codec, channels, sampleRate, bitsPerSample };
    } else if (id === 'data') {
      dataOffset = body;
      // A streamed file can carry 0xFFFFFFFF or a size past EOF.
      dataLength = Math.min(size, view.byteLength - body);
    }
    pos = body + size + (size % 2);
    if (size === 0) break;
  }

  if (!fmt) throw new Error('WAV file has no fmt chunk');
  if (dataOffset < 0) throw new Error('WAV file has no data chunk');
  const { codec, channels, sampleRate, bitsPerSample } = fmt;
  if (!channels || !sampleRate) throw new Error('WAV file has an invalid format chunk');

  const bytesPerSample =
    codec === WAVE_FORMAT_ALAW || codec === WAVE_FORMAT_MULAW ? 1 : Math.ceil(bitsPerSample / 8);
  const frameCount = Math.floor(dataLength / (bytesPerSample * channels));
  if (frameCount <= 0) throw new Error('WAV file contains no samples');

  const planes = [];
  for (let c = 0; c < channels; c++) planes.push(new Float32Array(frameCount));

  const readSample = (() => {
    if (codec === WAVE_FORMAT_FLOAT && bitsPerSample === 32) {
      return (off) => view.getFloat32(off, le);
    }
    if (codec === WAVE_FORMAT_FLOAT && bitsPerSample === 64) {
      return (off) => view.getFloat64(off, le);
    }
    if (codec === WAVE_FORMAT_ALAW) return (off) => alawToPcm(view.getUint8(off)) / 32768;
    if (codec === WAVE_FORMAT_MULAW) return (off) => mulawToPcm(view.getUint8(off)) / 32768;
    if (codec !== WAVE_FORMAT_PCM) {
      throw new Error(`Unsupported WAV codec 0x${codec.toString(16)}`);
    }
    if (bitsPerSample === 8) return (off) => (view.getUint8(off) - 128) / 128;
    if (bitsPerSample === 16) return (off) => view.getInt16(off, le) / 32768;
    if (bitsPerSample === 24) {
      return (off) => {
        const b0 = view.getUint8(off);
        const b1 = view.getUint8(off + 1);
        const b2 = view.getUint8(off + 2);
        const raw = le ? (b2 << 16) | (b1 << 8) | b0 : (b0 << 16) | (b1 << 8) | b2;
        return ((raw << 8) >> 8) / 8388608;
      };
    }
    if (bitsPerSample === 32) return (off) => view.getInt32(off, le) / 2147483648;
    throw new Error(`Unsupported WAV bit depth ${bitsPerSample}`);
  })();

  const stride = bytesPerSample * channels;
  for (let i = 0; i < frameCount; i++) {
    const frame = dataOffset + i * stride;
    for (let c = 0; c < channels; c++) {
      planes[c][i] = readSample(frame + c * bytesPerSample);
    }
  }

  return {
    channels: planes,
    sampleRate,
    bitsPerSample,
    codec,
    frameCount,
    duration: frameCount / sampleRate,
  };
}

/** Linearly resamples planar float channels to `targetRate`. */
export function resample(planes, sourceRate, targetRate) {
  if (sourceRate === targetRate) return planes;
  const ratio = targetRate / sourceRate;
  const outLength = Math.max(1, Math.round(planes[0].length * ratio));
  return planes.map((input) => {
    const out = new Float32Array(outLength);
    for (let i = 0; i < outLength; i++) {
      const src = i / ratio;
      const i0 = Math.floor(src);
      const i1 = Math.min(input.length - 1, i0 + 1);
      const t = src - i0;
      out[i] = input[i0] * (1 - t) + input[i1] * t;
    }
    return out;
  });
}

function toAudioBuffer(ctx, planes, rate) {
  const buffer = ctx.createBuffer(planes.length, planes[0].length, rate);
  for (let c = 0; c < planes.length; c++) buffer.copyToChannel(planes[c], c);
  return buffer;
}

/**
 * Decodes any supported audio payload into a playable AudioBuffer plus the
 * source file's true format, resampling when the device rate differs.
 */
export async function decodeAudio(ctx, arrayBuffer) {
  try {
    const wav = parseWav(arrayBuffer);
    const planes = resample(wav.channels, wav.sampleRate, ctx.sampleRate);
    return {
      buffer: toAudioBuffer(ctx, planes, ctx.sampleRate),
      sampleRate: wav.sampleRate,
      channelCount: wav.channels.length,
      bitsPerSample: wav.bitsPerSample,
      duration: wav.duration,
      resampled: wav.sampleRate !== ctx.sampleRate,
      container: 'wav',
    };
  } catch (wavError) {
    // Not RIFF (or an exotic RIFF codec) — hand it to the platform decoder,
    // which covers mp3 / m4a / ogg / flac / opus.
    try {
      const buffer = await ctx.decodeAudioData(arrayBuffer.slice(0));
      return {
        buffer,
        sampleRate: buffer.sampleRate,
        channelCount: buffer.numberOfChannels,
        bitsPerSample: null,
        duration: buffer.duration,
        resampled: false,
        container: 'compressed',
      };
    } catch (nativeError) {
      const detail = String(nativeError?.message || nativeError || '').trim();
      throw new Error(detail || String(wavError?.message || wavError));
    }
  }
}

/**
 * Reduces a channel to `bucketCount` min/max pairs for waveform drawing.
 */
export function computePeaks(buffer, bucketCount) {
  const buckets = Math.max(1, Math.floor(bucketCount));
  const channel = buffer.getChannelData(0);
  const second = buffer.numberOfChannels > 1 ? buffer.getChannelData(1) : null;
  const peaks = new Float32Array(buckets * 2);
  const step = channel.length / buckets;
  for (let b = 0; b < buckets; b++) {
    const start = Math.floor(b * step);
    const end = Math.min(channel.length, Math.floor((b + 1) * step)) || start + 1;
    let min = 0;
    let max = 0;
    for (let i = start; i < end; i++) {
      const v = second ? (channel[i] + second[i]) / 2 : channel[i];
      if (v < min) min = v;
      if (v > max) max = v;
    }
    peaks[b * 2] = min;
    peaks[b * 2 + 1] = max;
  }
  return peaks;
}
