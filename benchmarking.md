# Doradura Performance Comparison

## Test video
**URL:** https://youtu.be/ymFWgFiKUvM

## Metrics to compare
1. **Queue Time** — from sending the command to task start.
2. **Download Time** — from download start to completion.
3. **Upload Time** — from download completion to user receiving the file.
4. **Total Time** — from request to file delivery.
5. **File size** — MB; format MP3/MP4.
6. **Quality** — audio bitrate (kbps) or video resolution.

## Participants
1. **Doradura** — `/download https://youtu.be/ymFWgFiKUvM`; formats MP3/MP4; queue, rate limiting, plan priority.
2. **@youtubednbot** — `/start` → choose format → paste link; MP3/MP4.
3. **@YTfinderbot** — `/start` → paste link → choose format; MP3/MP4.

## Test procedure

### Preparation
- Ensure all bots are running and reachable.
- Have a stopwatch or use system time.
- Note baseline network state (ping, speed).

### Execution

#### Test 1: MP3 (320 kbps)
**Doradura:**
```
1) Send: /download https://youtu.be/ymFWgFiKUvM
2) Choose MP3, bitrate 320k
3) Measure times:
   T1: when "Added to queue" is received
   T2: when processing starts
   T3: when file is received
   Total = T3 - T1
```
**Competitors:** similar steps; measure first reply (T1) and file delivery (T2); Total = T2 - T1.

#### Test 2: MP4 (720p or best)
Use the same pattern; record resolution and file size.

### Reporting
For each bot/format, capture:
- Queue Time / Download Time / Upload Time / Total Time
- File size and quality
- Any errors or retries

## Notes
- Run multiple iterations to smooth out network variance.
- Keep cookies fresh for YouTube bots requiring auth.
- If a bot throttles or rate limits, note that in results.
