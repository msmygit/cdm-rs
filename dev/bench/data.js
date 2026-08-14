window.BENCHMARK_DATA = {
  "lastUpdate": 1786706584086,
  "repoUrl": "https://github.com/msmygit/cdm-rs",
  "entries": {
    "Benchmark": [
      {
        "commit": {
          "author": {
            "name": "Madhavan",
            "username": "msmygit",
            "email": "msmygit@users.noreply.github.com"
          },
          "committer": {
            "name": "GitHub",
            "username": "web-flow",
            "email": "noreply@github.com"
          },
          "id": "98eabf7fbc4fe1a3a54b2582712e5176906d48d0",
          "message": "Merge pull request #85 from msmygit/perf/tst-060-benchmarks\n\nperf: criterion micro-benchmarks for the hot path (TST-060)",
          "timestamp": "2026-08-14T10:22:44Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/98eabf7fbc4fe1a3a54b2582712e5176906d48d0"
        },
        "date": 1786706583089,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 27,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 27,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 27,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 59,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 174,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1762,
            "range": "± 9",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 29431,
            "range": "± 72",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 200,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2587,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 163408,
            "range": "± 615",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 205,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2588,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 163133,
            "range": "± 440",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 214,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 10185,
            "range": "± 18",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 10187,
            "range": "± 92",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 66,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 48,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/8",
            "value": 78,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/8",
            "value": 13,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/8",
            "value": 185,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/32",
            "value": 13,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/32",
            "value": 638,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/128",
            "value": 13,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/128",
            "value": 2556,
            "range": "± 19",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 97,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 309,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1194,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 302,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 268,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 321,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 6745,
            "range": "± 28",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 5065,
            "range": "± 75",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 27611,
            "range": "± 79",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 17391,
            "range": "± 323",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 120970,
            "range": "± 713",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 68285,
            "range": "± 387",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/4",
            "value": 78,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/16",
            "value": 202,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 777,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 127,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 273,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 852,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/4",
            "value": 12,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/16",
            "value": 12,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/64",
            "value": 12,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/4",
            "value": 12,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/16",
            "value": 12,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/64",
            "value": 12,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/none",
            "value": 42737,
            "range": "± 82",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 48907,
            "range": "± 425",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_adaptive_signal_overhead/disabled",
            "value": 2,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_adaptive_signal_overhead/enabled",
            "value": 41,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_no_features_baseline/empty_filter_chain",
            "value": 0,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_no_features_baseline/disabled_constant_columns",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/extend_target_binding",
            "value": 77,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 215,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 313,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 462,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2073,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2212,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 40378,
            "range": "± 390",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 41540,
            "range": "± 543",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept",
            "value": 14,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/reject",
            "value": 25,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept_chain_of_four",
            "value": 59,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 149,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2313,
            "range": "± 15",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 55178,
            "range": "± 198",
            "unit": "ns/iter"
          }
        ]
      }
    ]
  }
}