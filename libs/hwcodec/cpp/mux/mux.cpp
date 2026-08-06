// https://github.com/FFmpeg/FFmpeg/blob/master/doc/examples/muxing.c

extern "C" {
#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/opt.h>
#include <libavutil/timestamp.h>
}
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define LOG_MODULE "MUX"
#include <log.h>

namespace {
typedef struct OutputStream {
  AVStream *st;
  AVPacket *tmp_pkt;
} OutputStream;

class Muxer {
public:
  OutputStream video_st;
  // Opus audio track (48 kHz stereo, 10 ms frames). Mirrors the WebmRecorder
  // audio track in libs/scrap/src/common/record.rs so the same Opus stream
  // written by Recorder::write_audio works for both muxers.
  OutputStream audio_st;
  AVFormatContext *oc = NULL;
  int framerate;
  int64_t start_ms;
  int64_t last_pts;
  int got_first;
  int64_t audio_start_ms;
  int64_t audio_last_pts;
  int got_first_audio;

  Muxer() {}

  void destroy() {
    OutputStream *ost = &video_st;
    if (ost && ost->tmp_pkt)
      av_packet_free(&ost->tmp_pkt);
    OutputStream *ost_a = &audio_st;
    if (ost_a && ost_a->tmp_pkt)
      av_packet_free(&ost_a->tmp_pkt);
    if (oc && oc->pb && !(oc->oformat->flags & AVFMT_NOFILE))
      avio_closep(&oc->pb);
    if (oc)
      avformat_free_context(oc);
  }

  bool init(const char *filename, int width, int height, int is265,
            int framerate) {
    OutputStream *ost = &video_st;
    ost->st = NULL;
    ost->tmp_pkt = NULL;
    int ret;

    if ((ret = avformat_alloc_output_context2(&oc, NULL, NULL, filename)) < 0) {
          LOG_ERROR(std::string("avformat_alloc_output_context2 failed, ret = ") +
              std::to_string(ret));
      return false;
    }

    ost->st = avformat_new_stream(oc, NULL);
    if (!ost->st) {
      LOG_ERROR(std::string("avformat_new_stream failed"));
      return false;
    }
    ost->st->id = oc->nb_streams - 1;
    ost->st->codecpar->codec_id = is265 ? AV_CODEC_ID_H265 : AV_CODEC_ID_H264;
    ost->st->codecpar->codec_type = AVMEDIA_TYPE_VIDEO;
    ost->st->codecpar->width = width;
    ost->st->codecpar->height = height;

    if (!(oc->oformat->flags & AVFMT_NOFILE)) {
      ret = avio_open(&oc->pb, filename, AVIO_FLAG_WRITE);
      if (ret < 0) {
        LOG_ERROR(std::string("avio_open failed, ret = ") + std::to_string(ret));
        return false;
      }
    }

    ost->tmp_pkt = av_packet_alloc();
    if (!ost->tmp_pkt) {
      LOG_ERROR(std::string("av_packet_alloc failed"));
      return false;
    }

    // Audio stream: Opus passthrough (no re-encode). Created before
    // avformat_write_header so the track is part of the MP4 header.
    OutputStream *ost_a = &audio_st;
    ost_a->st = NULL;
    ost_a->tmp_pkt = NULL;
    ost_a->st = avformat_new_stream(oc, NULL);
    if (!ost_a->st) {
      LOG_ERROR(std::string("avformat_new_stream (audio) failed"));
      return false;
    }
    ost_a->st->id = oc->nb_streams - 1;
    ost_a->st->codecpar->codec_type = AVMEDIA_TYPE_AUDIO;
    ost_a->st->codecpar->codec_id = AV_CODEC_ID_OPUS;
    ost_a->st->codecpar->sample_rate = 48000;
#if LIBAVCODEC_VERSION_INT >= AV_VERSION_INT(59, 24, 100)
    av_channel_layout_default(&ost_a->st->codecpar->ch_layout, 2);
#else
    ost_a->st->codecpar->channels = 2;
    ost_a->st->codecpar->channel_layout = AV_CH_LAYOUT_STEREO;
#endif
    ost_a->st->time_base = AVRational{1, 48000};
    ost_a->tmp_pkt = av_packet_alloc();
    if (!ost_a->tmp_pkt) {
      LOG_ERROR(std::string("av_packet_alloc (audio) failed"));
      return false;
    }

    ret = avformat_write_header(oc, NULL);
    if (ret < 0) {
      LOG_ERROR(std::string("avformat_write_header failed"));
      return false;
    }

    this->framerate = framerate;
    this->start_ms = 0;
    this->last_pts = 0;
    this->got_first = 0;
    this->audio_start_ms = 0;
    this->audio_last_pts = 0;
    this->got_first_audio = 0;

    return true;
  }

  int write_video_frame(const uint8_t *data, int len, int64_t pts_ms, int key) {
    OutputStream *ost = &video_st;
    AVPacket *pkt = ost->tmp_pkt;
    AVFormatContext *fmt_ctx = oc;
    int ret;

    if (framerate <= 0)
      return -3;
    if (!got_first) {
      if (key != 1)
        return -2;
      start_ms = pts_ms;
    }
    int64_t pts = (pts_ms - start_ms); // use write timestamp
    if (pts <= last_pts && got_first) {
      pts = last_pts + 1000 / framerate;
    }
    got_first = 1;

    pkt->data = (uint8_t *)data;
    pkt->size = len;
    pkt->pts = pts;
    pkt->dts = pkt->pts; // no B-frame
    int64_t duration = pkt->pts - last_pts;
    last_pts = pkt->pts;
    pkt->duration = duration > 0 ? duration : 1000 / framerate; // predict
    AVRational rational;
    rational.num = 1;
    rational.den = 1000;
    av_packet_rescale_ts(pkt, rational,
                         ost->st->time_base); // ms -> stream timebase
    pkt->stream_index = ost->st->index;
    if (key == 1) {
      pkt->flags |= AV_PKT_FLAG_KEY;
    } else {
      pkt->flags &= ~AV_PKT_FLAG_KEY;
    }
    ret = av_write_frame(fmt_ctx, pkt);
    if (ret < 0) {
      LOG_ERROR(std::string("av_write_frame failed, ret = ") + std::to_string(ret));
      return -1;
    }
    return 0;
  }

  // Write a pre-encoded Opus packet (passthrough, no re-encode). pts_ms is on
  // the same wall clock as write_video_frame (ms since the muxer started), so
  // both tracks share one container timeline. 10 ms per Opus frame.
  int write_audio_frame(const uint8_t *data, int len, int64_t pts_ms) {
    OutputStream *ost = &audio_st;
    if (!ost->st)
      return -4;
    AVPacket *pkt = ost->tmp_pkt;
    AVFormatContext *fmt_ctx = oc;
    int ret;

    if (!got_first_audio) {
      audio_start_ms = pts_ms;
    }
    int64_t pts = pts_ms - audio_start_ms;
    if (pts <= audio_last_pts && got_first_audio) {
      pts = audio_last_pts + 10; // 10 ms per Opus frame, keep monotonic
    }
    got_first_audio = 1;
    audio_last_pts = pts;

    pkt->data = (uint8_t *)data;
    pkt->size = len;
    pkt->pts = pts;
    pkt->dts = pts;
    pkt->duration = 10; // ms
    pkt->flags &= ~AV_PKT_FLAG_KEY;
    AVRational rational;
    rational.num = 1;
    rational.den = 1000;
    av_packet_rescale_ts(pkt, rational, ost->st->time_base); // ms -> 48kHz
    pkt->stream_index = ost->st->index;
    ret = av_write_frame(fmt_ctx, pkt);
    if (ret < 0) {
      LOG_ERROR(std::string("av_write_frame (audio) failed, ret = ") + std::to_string(ret));
      return -1;
    }
    return 0;
  }
};
} // namespace

extern "C" Muxer *hwcodec_new_muxer(const char *filename, int width, int height,
                                    int is265, int framerate) {
  Muxer *muxer = NULL;
  try {
    muxer = new Muxer();
    if (muxer) {
      if (muxer->init(filename, width, height, is265, framerate)) {
        return muxer;
      }
    }
  } catch (const std::exception &e) {
    LOG_ERROR(std::string("new muxer exception: ") + std::string(e.what()));
  }
  if (muxer) {
    muxer->destroy();
    delete muxer;
    muxer = NULL;
  }
  return NULL;
}

extern "C" int hwcodec_write_video_frame(Muxer *muxer, const uint8_t *data,
                                         int len, int64_t pts_ms, int key) {
  try {
    return muxer->write_video_frame(data, len, pts_ms, key);
  } catch (const std::exception &e) {
    LOG_ERROR(std::string("write_video_frame exception: ") + std::string(e.what()));
  }
  return -1;
}

extern "C" int hwcodec_write_audio_frame(Muxer *muxer, const uint8_t *data,
                                        int len, int64_t pts_ms) {
  try {
    return muxer->write_audio_frame(data, len, pts_ms);
  } catch (const std::exception &e) {
    LOG_ERROR(std::string("write_audio_frame exception: ") + std::string(e.what()));
  }
  return -1;
}

extern "C" int hwcodec_write_tail(Muxer *muxer) {
  return av_write_trailer(muxer->oc);
}

extern "C" void hwcodec_free_muxer(Muxer *muxer) {
  try {
    if (!muxer)
      return;
    muxer->destroy();
    delete muxer;
    muxer = NULL;
  } catch (const std::exception &e) {
    LOG_ERROR(std::string("free_muxer exception: ") + std::string(e.what()));
  }
}