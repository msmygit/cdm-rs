window.BENCHMARK_DATA = {
  "lastUpdate": 1789030671260,
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
      },
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
          "id": "67ce13c74a077a8ab58eb6ef32a47d82735dfee5",
          "message": "Merge pull request #91 from msmygit/docs/ci-gates\n\ndocs: record the full CI gate list in AGENTS.md",
          "timestamp": "2026-08-15T01:28:49Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/67ce13c74a077a8ab58eb6ef32a47d82735dfee5"
        },
        "date": 1786769289097,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 54,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 165,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1792,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 28631,
            "range": "± 228",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 187,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2340,
            "range": "± 65",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 145030,
            "range": "± 754",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 192,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2325,
            "range": "± 14",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 145143,
            "range": "± 400",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 192,
            "range": "± 8",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 9110,
            "range": "± 31",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 9110,
            "range": "± 36",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 32,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 44,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/8",
            "value": 72,
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
            "value": 154,
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
            "value": 568,
            "range": "± 3",
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
            "value": 2235,
            "range": "± 16",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 91,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 291,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1142,
            "range": "± 16",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 288,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 253,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 312,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 6910,
            "range": "± 24",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 5027,
            "range": "± 21",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 28966,
            "range": "± 132",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 18796,
            "range": "± 89",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 122238,
            "range": "± 883",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 68854,
            "range": "± 2812",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/4",
            "value": 55,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/16",
            "value": 207,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 844,
            "range": "± 13",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 106,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 281,
            "range": "± 8",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 956,
            "range": "± 17",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/4",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/16",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/64",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/4",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/16",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/64",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/none",
            "value": 39761,
            "range": "± 65",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 44084,
            "range": "± 123",
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
            "value": 39,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/64",
            "value": 875,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 11268,
            "range": "± 39",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 724537,
            "range": "± 2543",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 87452,
            "range": "± 165",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 87548,
            "range": "± 686",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 88085,
            "range": "± 1135",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 5805192,
            "range": "± 18157",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 37,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 789,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 22399,
            "range": "± 36",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 739776,
            "range": "± 3745",
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
            "value": 9,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/extend_target_binding",
            "value": 73,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 216,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 312,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 481,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2020,
            "range": "± 8",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2267,
            "range": "± 20",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 44564,
            "range": "± 258",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 46562,
            "range": "± 474",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept",
            "value": 15,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/reject",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept_chain_of_four",
            "value": 60,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 143,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2127,
            "range": "± 43",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 52443,
            "range": "± 305",
            "unit": "ns/iter"
          }
        ]
      },
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
          "id": "67ce13c74a077a8ab58eb6ef32a47d82735dfee5",
          "message": "Merge pull request #91 from msmygit/docs/ci-gates\n\ndocs: record the full CI gate list in AGENTS.md",
          "timestamp": "2026-08-15T01:28:49Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/67ce13c74a077a8ab58eb6ef32a47d82735dfee5"
        },
        "date": 1786942762082,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 56,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 164,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1846,
            "range": "± 13",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 28907,
            "range": "± 216",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 187,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2293,
            "range": "± 9",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 145308,
            "range": "± 495",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 196,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2296,
            "range": "± 77",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 145478,
            "range": "± 5365",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 180,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 9062,
            "range": "± 213",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 9056,
            "range": "± 303",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 32,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 44,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/8",
            "value": 72,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/8",
            "value": 12,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/8",
            "value": 159,
            "range": "± 11",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/32",
            "value": 12,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/32",
            "value": 566,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/128",
            "value": 11,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/128",
            "value": 2241,
            "range": "± 77",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 91,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 285,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1106,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 288,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 253,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 314,
            "range": "± 22",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 6720,
            "range": "± 20",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 5157,
            "range": "± 166",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 27938,
            "range": "± 236",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 18827,
            "range": "± 81",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 123666,
            "range": "± 673",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 68750,
            "range": "± 1032",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/4",
            "value": 54,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/16",
            "value": 232,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 939,
            "range": "± 11",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 106,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 267,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 941,
            "range": "± 11",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/4",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/16",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/64",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/4",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/16",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/64",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/none",
            "value": 39767,
            "range": "± 48",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 44471,
            "range": "± 657",
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
            "value": 39,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/64",
            "value": 999,
            "range": "± 9",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 11731,
            "range": "± 354",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 719300,
            "range": "± 3692",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 88309,
            "range": "± 176",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 88353,
            "range": "± 338",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 88356,
            "range": "± 3198",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 5813464,
            "range": "± 24253",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 37,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 762,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 22436,
            "range": "± 391",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 746472,
            "range": "± 4472",
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
            "value": 9,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/extend_target_binding",
            "value": 82,
            "range": "± 17",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 225,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 312,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 486,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2046,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2273,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 46760,
            "range": "± 411",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 47140,
            "range": "± 352",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept",
            "value": 15,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/reject",
            "value": 22,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept_chain_of_four",
            "value": 60,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 149,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2129,
            "range": "± 28",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 52831,
            "range": "± 1457",
            "unit": "ns/iter"
          }
        ]
      },
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
          "id": "72b38003bb33518bd7e89c34457eb88b6b5b93e0",
          "message": "Merge pull request #92 from msmygit/dependabot/github_actions/taiki-e/install-action-2.85.13\n\nchore(deps): bump taiki-e/install-action from 2.85.9 to 2.85.13",
          "timestamp": "2026-08-19T01:27:21Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/72b38003bb33518bd7e89c34457eb88b6b5b93e0"
        },
        "date": 1787115184959,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 21,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 21,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 21,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 46,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 140,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1350,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 23228,
            "range": "± 98",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 171,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2048,
            "range": "± 49",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 126384,
            "range": "± 9883",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 175,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2034,
            "range": "± 27",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 126333,
            "range": "± 2050",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 167,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 7982,
            "range": "± 154",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 7985,
            "range": "± 22",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 51,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 37,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/8",
            "value": 60,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/8",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/8",
            "value": 137,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/32",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/32",
            "value": 495,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/128",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/128",
            "value": 1962,
            "range": "± 16",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 75,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 233,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 953,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 235,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 209,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 245,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 5389,
            "range": "± 16",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 3963,
            "range": "± 128",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 22485,
            "range": "± 114",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 13840,
            "range": "± 71",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 96411,
            "range": "± 1494",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 53449,
            "range": "± 140",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/4",
            "value": 44,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/16",
            "value": 155,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 604,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 93,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 202,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 667,
            "range": "± 21",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/4",
            "value": 9,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/16",
            "value": 9,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/64",
            "value": 9,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/4",
            "value": 9,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/16",
            "value": 9,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/64",
            "value": 9,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/none",
            "value": 33060,
            "range": "± 367",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 37790,
            "range": "± 391",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_adaptive_signal_overhead/disabled",
            "value": 1,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_adaptive_signal_overhead/enabled",
            "value": 32,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/64",
            "value": 690,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 9545,
            "range": "± 25",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 576991,
            "range": "± 1797",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 71377,
            "range": "± 254",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 71060,
            "range": "± 1677",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 70985,
            "range": "± 251",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 4343847,
            "range": "± 188827",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 30,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 1062,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 11510,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 668746,
            "range": "± 3247",
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
            "value": 8,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/extend_target_binding",
            "value": 61,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 166,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 248,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 365,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 1553,
            "range": "± 129",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 1671,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 30185,
            "range": "± 76",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 30185,
            "range": "± 63",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept",
            "value": 11,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/reject",
            "value": 19,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept_chain_of_four",
            "value": 46,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 117,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 1826,
            "range": "± 12",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 42474,
            "range": "± 158",
            "unit": "ns/iter"
          }
        ]
      },
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
          "id": "72b38003bb33518bd7e89c34457eb88b6b5b93e0",
          "message": "Merge pull request #92 from msmygit/dependabot/github_actions/taiki-e/install-action-2.85.13\n\nchore(deps): bump taiki-e/install-action from 2.85.9 to 2.85.13",
          "timestamp": "2026-08-19T01:27:21Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/72b38003bb33518bd7e89c34457eb88b6b5b93e0"
        },
        "date": 1787287971907,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 23,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 23,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 36,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 107,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1289,
            "range": "± 43",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 20960,
            "range": "± 1227",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 123,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 1477,
            "range": "± 68",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 102166,
            "range": "± 4846",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 135,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 1455,
            "range": "± 12",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 103449,
            "range": "± 2087",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 123,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 6010,
            "range": "± 16",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 6024,
            "range": "± 254",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 28,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 51,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/8",
            "value": 103,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/8",
            "value": 4,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/8",
            "value": 149,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/32",
            "value": 3,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/32",
            "value": 592,
            "range": "± 30",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/128",
            "value": 3,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/128",
            "value": 2316,
            "range": "± 17",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 56,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 177,
            "range": "± 11",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 727,
            "range": "± 32",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 184,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 166,
            "range": "± 9",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 196,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 5583,
            "range": "± 178",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 3944,
            "range": "± 100",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 21537,
            "range": "± 533",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 14315,
            "range": "± 378",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 88461,
            "range": "± 160",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 52708,
            "range": "± 127",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/4",
            "value": 56,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/16",
            "value": 216,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 861,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 106,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 265,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 907,
            "range": "± 52",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/4",
            "value": 2,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/16",
            "value": 2,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/64",
            "value": 2,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/4",
            "value": 2,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/16",
            "value": 2,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/64",
            "value": 2,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/none",
            "value": 24215,
            "range": "± 137",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 41393,
            "range": "± 170",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_adaptive_signal_overhead/disabled",
            "value": 1,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_adaptive_signal_overhead/enabled",
            "value": 32,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/64",
            "value": 511,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 6970,
            "range": "± 16",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 436165,
            "range": "± 935",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 52082,
            "range": "± 81",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 59236,
            "range": "± 3098",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 52389,
            "range": "± 2507",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 3215903,
            "range": "± 24184",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 24,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 667,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 10471,
            "range": "± 475",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 773366,
            "range": "± 26571",
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
            "value": 5,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/extend_target_binding",
            "value": 42,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 119,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 179,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 275,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 1365,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 1499,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 25832,
            "range": "± 1019",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 25940,
            "range": "± 69",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept",
            "value": 11,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/reject",
            "value": 16,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept_chain_of_four",
            "value": 47,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 114,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2673,
            "range": "± 82",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 51581,
            "range": "± 1892",
            "unit": "ns/iter"
          }
        ]
      },
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
          "id": "72b38003bb33518bd7e89c34457eb88b6b5b93e0",
          "message": "Merge pull request #92 from msmygit/dependabot/github_actions/taiki-e/install-action-2.85.13\n\nchore(deps): bump taiki-e/install-action from 2.85.9 to 2.85.13",
          "timestamp": "2026-08-19T01:27:21Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/72b38003bb33518bd7e89c34457eb88b6b5b93e0"
        },
        "date": 1788511719761,
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
            "value": 184,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1757,
            "range": "± 126",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 29653,
            "range": "± 693",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 201,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2585,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 163358,
            "range": "± 431",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 206,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2587,
            "range": "± 52",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 163378,
            "range": "± 663",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 227,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 10211,
            "range": "± 125",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 10204,
            "range": "± 28",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 63,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 49,
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
            "value": 175,
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
            "value": 637,
            "range": "± 10",
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
            "value": 2566,
            "range": "± 36",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 98,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 301,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1193,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 306,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 274,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 319,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 6777,
            "range": "± 30",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 5066,
            "range": "± 26",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 27561,
            "range": "± 108",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 17437,
            "range": "± 47",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 117962,
            "range": "± 4145",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 71152,
            "range": "± 232",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/4",
            "value": 57,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/16",
            "value": 201,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 772,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 123,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 265,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 842,
            "range": "± 2",
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
            "value": 43518,
            "range": "± 465",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 51108,
            "range": "± 750",
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
            "name": "tst_060_split_ring_murmur3/64",
            "value": 893,
            "range": "± 23",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 11977,
            "range": "± 796",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 750864,
            "range": "± 14893",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 92147,
            "range": "± 299",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 92109,
            "range": "± 1142",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 92084,
            "range": "± 5559",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 5699005,
            "range": "± 66742",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 39,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 1367,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 14747,
            "range": "± 1500",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 865862,
            "range": "± 11226",
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
            "value": 76,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 214,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 317,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 472,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2007,
            "range": "± 16",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2175,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 39657,
            "range": "± 116",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 40155,
            "range": "± 153",
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
            "value": 58,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 157,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2457,
            "range": "± 11",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 55338,
            "range": "± 917",
            "unit": "ns/iter"
          }
        ]
      },
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
          "id": "72b38003bb33518bd7e89c34457eb88b6b5b93e0",
          "message": "Merge pull request #92 from msmygit/dependabot/github_actions/taiki-e/install-action-2.85.13\n\nchore(deps): bump taiki-e/install-action from 2.85.9 to 2.85.13",
          "timestamp": "2026-08-19T01:27:21Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/72b38003bb33518bd7e89c34457eb88b6b5b93e0"
        },
        "date": 1788772549141,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 56,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 164,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1729,
            "range": "± 66",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 28428,
            "range": "± 125",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 187,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2290,
            "range": "± 8",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 145452,
            "range": "± 1044",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 190,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2289,
            "range": "± 33",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 145114,
            "range": "± 1028",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 194,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 9026,
            "range": "± 249",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 9028,
            "range": "± 116",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 32,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 44,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/8",
            "value": 73,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/8",
            "value": 11,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/8",
            "value": 153,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/32",
            "value": 11,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/32",
            "value": 565,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/128",
            "value": 11,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/128",
            "value": 2237,
            "range": "± 36",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 92,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 294,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1126,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 294,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 250,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 322,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 6849,
            "range": "± 42",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 5072,
            "range": "± 97",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 27812,
            "range": "± 243",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 18692,
            "range": "± 405",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 118942,
            "range": "± 4855",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 71711,
            "range": "± 319",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/4",
            "value": 53,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/16",
            "value": 193,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 807,
            "range": "± 21",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 104,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 244,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 863,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/4",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/16",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/64",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/4",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/16",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/64",
            "value": 10,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/none",
            "value": 39333,
            "range": "± 1378",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 44971,
            "range": "± 313",
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
            "value": 38,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/64",
            "value": 937,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 11634,
            "range": "± 166",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 722241,
            "range": "± 6267",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 87799,
            "range": "± 281",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 88106,
            "range": "± 1830",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 88104,
            "range": "± 291",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 5778091,
            "range": "± 30807",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 37,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 778,
            "range": "± 25",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 22575,
            "range": "± 56",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 745065,
            "range": "± 14354",
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
            "value": 9,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/extend_target_binding",
            "value": 72,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 231,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 311,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 476,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2029,
            "range": "± 58",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2241,
            "range": "± 41",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 45114,
            "range": "± 216",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 46746,
            "range": "± 199",
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
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept_chain_of_four",
            "value": 58,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 143,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2120,
            "range": "± 58",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 53047,
            "range": "± 376",
            "unit": "ns/iter"
          }
        ]
      },
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
          "id": "2dfd93258211e65337cab751af93f5d26e07018f",
          "message": "Merge pull request #97 from msmygit/dependabot/github_actions/actions/setup-java-6.0.0\n\nchore(deps): bump actions/setup-java from 5.7.0 to 6.0.0",
          "timestamp": "2026-09-09T17:31:43Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/2dfd93258211e65337cab751af93f5d26e07018f"
        },
        "date": 1789030670420,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 23,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 23,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 36,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 109,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1296,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 21073,
            "range": "± 1417",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 133,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 1469,
            "range": "± 46",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 99455,
            "range": "± 3180",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 131,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 1462,
            "range": "± 13",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 110680,
            "range": "± 4641",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 131,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 6086,
            "range": "± 308",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 6028,
            "range": "± 126",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 28,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 49,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/8",
            "value": 103,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/8",
            "value": 3,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/8",
            "value": 149,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/32",
            "value": 3,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/32",
            "value": 570,
            "range": "± 22",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/128",
            "value": 3,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/128",
            "value": 2422,
            "range": "± 126",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 59,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 217,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 786,
            "range": "± 30",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 204,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 169,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 277,
            "range": "± 18",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 5585,
            "range": "± 38",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 3494,
            "range": "± 89",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 22011,
            "range": "± 1005",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 13507,
            "range": "± 777",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 89266,
            "range": "± 6341",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 49487,
            "range": "± 439",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/4",
            "value": 59,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/16",
            "value": 216,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 862,
            "range": "± 22",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 107,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 270,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 905,
            "range": "± 33",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/4",
            "value": 2,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/16",
            "value": 2,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/64",
            "value": 2,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/4",
            "value": 2,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/16",
            "value": 2,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/64",
            "value": 2,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/none",
            "value": 24194,
            "range": "± 563",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 41364,
            "range": "± 1148",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_adaptive_signal_overhead/disabled",
            "value": 1,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_adaptive_signal_overhead/enabled",
            "value": 32,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/64",
            "value": 577,
            "range": "± 27",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 6802,
            "range": "± 296",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 438902,
            "range": "± 19118",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 52296,
            "range": "± 1802",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 52342,
            "range": "± 329",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 58075,
            "range": "± 3471",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 3216867,
            "range": "± 114710",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 24,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 676,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 10559,
            "range": "± 685",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 758854,
            "range": "± 15658",
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
            "value": 5,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/extend_target_binding",
            "value": 41,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 124,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 176,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 273,
            "range": "± 15",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 1377,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 1502,
            "range": "± 65",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 25558,
            "range": "± 707",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 25643,
            "range": "± 268",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept",
            "value": 12,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/reject",
            "value": 14,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept_chain_of_four",
            "value": 42,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 114,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2610,
            "range": "± 103",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 50465,
            "range": "± 2729",
            "unit": "ns/iter"
          }
        ]
      }
    ]
  }
}