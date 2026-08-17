window.BENCHMARK_DATA = {
  "lastUpdate": 1786942763025,
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
      }
    ]
  }
}