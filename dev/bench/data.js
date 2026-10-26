window.BENCHMARK_DATA = {
  "lastUpdate": 1793008968963,
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
        "date": 1789379682008,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 36,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 33,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 33,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 50,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 153,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1870,
            "range": "± 13",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 28819,
            "range": "± 162",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 162,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2084,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 154406,
            "range": "± 14511",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 173,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2088,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 152592,
            "range": "± 13899",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 174,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 8870,
            "range": "± 910",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 8867,
            "range": "± 912",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 40,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 75,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/8",
            "value": 163,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/8",
            "value": 5,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/8",
            "value": 218,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/32",
            "value": 5,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/32",
            "value": 838,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/128",
            "value": 5,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/128",
            "value": 3382,
            "range": "± 26",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 81,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 278,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1144,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 263,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 221,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 309,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 8005,
            "range": "± 42",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 5350,
            "range": "± 36",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 30949,
            "range": "± 245",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 18461,
            "range": "± 130",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 130051,
            "range": "± 643",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 73636,
            "range": "± 748",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/4",
            "value": 88,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/16",
            "value": 340,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 1347,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 162,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 416,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 1415,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/4",
            "value": 3,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/16",
            "value": 3,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_missing_target_row/64",
            "value": 3,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/4",
            "value": 3,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/16",
            "value": 3,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/64",
            "value": 3,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/none",
            "value": 33063,
            "range": "± 153",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 60224,
            "range": "± 292",
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
            "value": 46,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/64",
            "value": 729,
            "range": "± 12",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 9806,
            "range": "± 76",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 621541,
            "range": "± 3152",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 74993,
            "range": "± 443",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 75093,
            "range": "± 334",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 75322,
            "range": "± 1176",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 4666410,
            "range": "± 38905",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 35,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 911,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 14432,
            "range": "± 61",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 1060327,
            "range": "± 18314",
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
            "value": 65,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 180,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 261,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 409,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2100,
            "range": "± 13",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2266,
            "range": "± 11",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 40507,
            "range": "± 392",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 40141,
            "range": "± 393",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept",
            "value": 16,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/reject",
            "value": 20,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept_chain_of_four",
            "value": 65,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 163,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 3759,
            "range": "± 39",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 74727,
            "range": "± 362",
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
        "date": 1789984581141,
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
            "value": 55,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 163,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1712,
            "range": "± 11",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 28434,
            "range": "± 245",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 187,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2291,
            "range": "± 17",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 145699,
            "range": "± 1524",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 180,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2290,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 145566,
            "range": "± 615",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 192,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 9007,
            "range": "± 57",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 9007,
            "range": "± 33",
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
            "value": 562,
            "range": "± 4",
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
            "value": 2231,
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
            "value": 294,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1123,
            "range": "± 39",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 294,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 250,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 322,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 6920,
            "range": "± 138",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 5171,
            "range": "± 32",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 27938,
            "range": "± 496",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 18221,
            "range": "± 54",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 119439,
            "range": "± 1138",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 68893,
            "range": "± 1043",
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
            "value": 200,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 863,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 103,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 245,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 845,
            "range": "± 3",
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
            "value": 39302,
            "range": "± 120",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 44797,
            "range": "± 294",
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
            "value": 870,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 11455,
            "range": "± 35",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 720156,
            "range": "± 29075",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 88503,
            "range": "± 1332",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 89055,
            "range": "± 565",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 89062,
            "range": "± 429",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 5756720,
            "range": "± 50870",
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
            "value": 760,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 22989,
            "range": "± 43",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 728292,
            "range": "± 3442",
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
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 215,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 315,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 480,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2076,
            "range": "± 11",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2265,
            "range": "± 20",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 41762,
            "range": "± 242",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 43649,
            "range": "± 107",
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
            "value": 22,
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
            "value": 143,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2163,
            "range": "± 19",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 53220,
            "range": "± 432",
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
          "id": "4f327b03321faf509d841e50755e340a0064a944",
          "message": "Merge pull request #106 from msmygit/dependabot/github_actions/taiki-e/install-action-2.87.20\n\nchore(deps): bump taiki-e/install-action from 2.85.13 to 2.87.20",
          "timestamp": "2026-09-28T01:21:55Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/4f327b03321faf509d841e50755e340a0064a944"
        },
        "date": 1790592796946,
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
            "value": 45,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 138,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1389,
            "range": "± 34",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 22724,
            "range": "± 207",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 155,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2007,
            "range": "± 126",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 126566,
            "range": "± 392",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 162,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2005,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 126459,
            "range": "± 561",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 158,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 7913,
            "range": "± 32",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 7913,
            "range": "± 188",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 49,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 39,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/8",
            "value": 76,
            "range": "± 5",
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
            "value": 135,
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
            "value": 494,
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
            "value": 1986,
            "range": "± 19",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 75,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 233,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 945,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 238,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 213,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 247,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 5474,
            "range": "± 49",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 3858,
            "range": "± 33",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 22076,
            "range": "± 43",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 14363,
            "range": "± 111",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 95407,
            "range": "± 418",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 57175,
            "range": "± 612",
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
            "value": 154,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 609,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 92,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 202,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 653,
            "range": "± 1",
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
            "value": 33079,
            "range": "± 63",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 37788,
            "range": "± 483",
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
            "value": 693,
            "range": "± 9",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 9296,
            "range": "± 213",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 580780,
            "range": "± 1259",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 71690,
            "range": "± 128",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 71653,
            "range": "± 1094",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 71721,
            "range": "± 175",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 4391704,
            "range": "± 41924",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 30,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 1067,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 11842,
            "range": "± 16",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 671961,
            "range": "± 4559",
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
            "value": 59,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 170,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 246,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 365,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 1514,
            "range": "± 32",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 1641,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 28755,
            "range": "± 105",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 28854,
            "range": "± 1139",
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
            "value": 20,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept_chain_of_four",
            "value": 48,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 114,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 1818,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 42967,
            "range": "± 1606",
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
          "id": "4f327b03321faf509d841e50755e340a0064a944",
          "message": "Merge pull request #106 from msmygit/dependabot/github_actions/taiki-e/install-action-2.87.20\n\nchore(deps): bump taiki-e/install-action from 2.85.13 to 2.87.20",
          "timestamp": "2026-09-28T01:21:55Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/4f327b03321faf509d841e50755e340a0064a944"
        },
        "date": 1790937528181,
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
            "value": 20,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 20,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 54,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 163,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1784,
            "range": "± 12",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 30532,
            "range": "± 150",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 191,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2297,
            "range": "± 26",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 145907,
            "range": "± 1016",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 197,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2325,
            "range": "± 22",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 146031,
            "range": "± 666",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 193,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 9060,
            "range": "± 161",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 9068,
            "range": "± 167",
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
            "range": "± 1",
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
            "value": 13,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/8",
            "value": 137,
            "range": "± 3",
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
            "value": 490,
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
            "value": 1968,
            "range": "± 21",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 89,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 281,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1095,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 282,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 257,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 314,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 6432,
            "range": "± 24",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 4876,
            "range": "± 19",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 25896,
            "range": "± 130",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 17388,
            "range": "± 85",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 115437,
            "range": "± 574",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 64615,
            "range": "± 760",
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
            "value": 215,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 819,
            "range": "± 38",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 104,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 270,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 919,
            "range": "± 5",
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
            "value": 38041,
            "range": "± 71",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 45102,
            "range": "± 112",
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
            "value": 866,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 11484,
            "range": "± 49",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 720482,
            "range": "± 3793",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 87842,
            "range": "± 258",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 87749,
            "range": "± 1216",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 87780,
            "range": "± 619",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 5768906,
            "range": "± 28285",
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
            "value": 760,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 24432,
            "range": "± 136",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 724051,
            "range": "± 4173",
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
            "value": 70,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 216,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 307,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 457,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2023,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2226,
            "range": "± 20",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 42015,
            "range": "± 541",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 44196,
            "range": "± 158",
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
            "value": 138,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2105,
            "range": "± 22",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 51827,
            "range": "± 375",
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
          "id": "4f327b03321faf509d841e50755e340a0064a944",
          "message": "Merge pull request #106 from msmygit/dependabot/github_actions/taiki-e/install-action-2.87.20\n\nchore(deps): bump taiki-e/install-action from 2.85.13 to 2.87.20",
          "timestamp": "2026-09-28T01:21:55Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/4f327b03321faf509d841e50755e340a0064a944"
        },
        "date": 1791199854860,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 31,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 31,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 31,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 51,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 154,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 2295,
            "range": "± 13",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 28481,
            "range": "± 121",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 163,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2111,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 153982,
            "range": "± 15015",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 184,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2112,
            "range": "± 9",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 151951,
            "range": "± 14158",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 183,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 9052,
            "range": "± 938",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 9045,
            "range": "± 914",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 40,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 75,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/8",
            "value": 166,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/8",
            "value": 5,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/8",
            "value": 202,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/32",
            "value": 5,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/32",
            "value": 779,
            "range": "± 9",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/128",
            "value": 5,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/128",
            "value": 3129,
            "range": "± 23",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 72,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 237,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 962,
            "range": "± 22",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 234,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 238,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 261,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 7955,
            "range": "± 23",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 5021,
            "range": "± 15",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 30859,
            "range": "± 282",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 18202,
            "range": "± 51",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 129440,
            "range": "± 1335",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 69571,
            "range": "± 320",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/4",
            "value": 91,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/16",
            "value": 352,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 1428,
            "range": "± 34",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 167,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 422,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 1495,
            "range": "± 3",
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
            "value": 3,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/16",
            "value": 3,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_keys_only/64",
            "value": 3,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/none",
            "value": 33066,
            "range": "± 294",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 60188,
            "range": "± 107",
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
            "value": 46,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/64",
            "value": 732,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 9854,
            "range": "± 104",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 627895,
            "range": "± 6165",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 75744,
            "range": "± 173",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 75947,
            "range": "± 198",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 75972,
            "range": "± 243",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 4713953,
            "range": "± 37978",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 35,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 907,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 14472,
            "range": "± 63",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 1005243,
            "range": "± 28036",
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
            "value": 66,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 179,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 260,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 408,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2105,
            "range": "± 20",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2285,
            "range": "± 8",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 39502,
            "range": "± 151",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 39579,
            "range": "± 906",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept",
            "value": 18,
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
            "value": 72,
            "range": "± 1",
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
            "value": 3545,
            "range": "± 21",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 74243,
            "range": "± 882",
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
          "id": "4f327b03321faf509d841e50755e340a0064a944",
          "message": "Merge pull request #106 from msmygit/dependabot/github_actions/taiki-e/install-action-2.87.20\n\nchore(deps): bump taiki-e/install-action from 2.85.13 to 2.87.20",
          "timestamp": "2026-09-28T01:21:55Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/4f327b03321faf509d841e50755e340a0064a944"
        },
        "date": 1791806384944,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 20,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 20,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 20,
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
            "value": 164,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1815,
            "range": "± 16",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 28873,
            "range": "± 96",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 191,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2319,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 145291,
            "range": "± 958",
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
            "value": 2293,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 145338,
            "range": "± 467",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 194,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 9103,
            "range": "± 186",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 9099,
            "range": "± 24",
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
            "value": 13,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/8",
            "value": 136,
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
            "value": 483,
            "range": "± 5",
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
            "value": 1977,
            "range": "± 21",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 89,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 282,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1091,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 282,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 257,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 312,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 6458,
            "range": "± 25",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 4936,
            "range": "± 22",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 26886,
            "range": "± 147",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 17228,
            "range": "± 384",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 115003,
            "range": "± 645",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 67378,
            "range": "± 283",
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
            "value": 206,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 912,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 104,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 257,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 926,
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
            "value": 38036,
            "range": "± 62",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 45082,
            "range": "± 95",
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
            "value": 942,
            "range": "± 26",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 11258,
            "range": "± 49",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 720271,
            "range": "± 1890",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 88158,
            "range": "± 270",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 87977,
            "range": "± 374",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 87986,
            "range": "± 295",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 5764601,
            "range": "± 17620",
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
            "value": 759,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 22396,
            "range": "± 42",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 747193,
            "range": "± 2669",
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
            "value": 71,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 218,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 320,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 467,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2049,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2266,
            "range": "± 12",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 42257,
            "range": "± 180",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 43753,
            "range": "± 153",
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
            "value": 139,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2156,
            "range": "± 11",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 51280,
            "range": "± 145",
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
          "id": "4f327b03321faf509d841e50755e340a0064a944",
          "message": "Merge pull request #106 from msmygit/dependabot/github_actions/taiki-e/install-action-2.87.20\n\nchore(deps): bump taiki-e/install-action from 2.85.13 to 2.87.20",
          "timestamp": "2026-09-28T01:21:55Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/4f327b03321faf509d841e50755e340a0064a944"
        },
        "date": 1792141524699,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 20,
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
            "value": 54,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 163,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1735,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 28249,
            "range": "± 80",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 190,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2344,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 145851,
            "range": "± 1218",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 205,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2318,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 145356,
            "range": "± 1303",
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
            "value": 9056,
            "range": "± 45",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 9057,
            "range": "± 23",
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
            "value": 134,
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
            "value": 486,
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
            "value": 1967,
            "range": "± 20",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 89,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 282,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1096,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 286,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 259,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 313,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 6531,
            "range": "± 27",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 5012,
            "range": "± 21",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 27272,
            "range": "± 145",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 17139,
            "range": "± 81",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 113892,
            "range": "± 522",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 68152,
            "range": "± 625",
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
            "value": 210,
            "range": "± 8",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 887,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 103,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 271,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 893,
            "range": "± 3",
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
            "value": 38031,
            "range": "± 48",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 45081,
            "range": "± 1181",
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
            "value": 906,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 11551,
            "range": "± 145",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 736255,
            "range": "± 2696",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 89120,
            "range": "± 155",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 89008,
            "range": "± 403",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 89014,
            "range": "± 1925",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 5680916,
            "range": "± 26255",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 38,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 759,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 22403,
            "range": "± 56",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 735033,
            "range": "± 9600",
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
            "value": 71,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 215,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 315,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 475,
            "range": "± 10",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2009,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2278,
            "range": "± 14",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 44912,
            "range": "± 224",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 45231,
            "range": "± 134",
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
            "value": 138,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2159,
            "range": "± 36",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 51176,
            "range": "± 260",
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
          "id": "4f327b03321faf509d841e50755e340a0064a944",
          "message": "Merge pull request #106 from msmygit/dependabot/github_actions/taiki-e/install-action-2.87.20\n\nchore(deps): bump taiki-e/install-action from 2.85.13 to 2.87.20",
          "timestamp": "2026-09-28T01:21:55Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/4f327b03321faf509d841e50755e340a0064a944"
        },
        "date": 1792403739089,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 20,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 20,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 20,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 55,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 163,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1775,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 28431,
            "range": "± 178",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 214,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2303,
            "range": "± 34",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 146377,
            "range": "± 1440",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 206,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2300,
            "range": "± 17",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 146202,
            "range": "± 1264",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 201,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 9107,
            "range": "± 126",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 9116,
            "range": "± 126",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 32,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 46,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/8",
            "value": 73,
            "range": "± 1",
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
            "value": 134,
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
            "value": 485,
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
            "value": 1991,
            "range": "± 17",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 90,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 282,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1102,
            "range": "± 16",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 287,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 260,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 312,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 6476,
            "range": "± 58",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 4787,
            "range": "± 35",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 27688,
            "range": "± 222",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 17361,
            "range": "± 138",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 112837,
            "range": "± 1259",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 64937,
            "range": "± 405",
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
            "value": 211,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 839,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 108,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 268,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 918,
            "range": "± 20",
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
            "value": 38042,
            "range": "± 164",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 45086,
            "range": "± 86",
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
            "value": 909,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 11568,
            "range": "± 162",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 732817,
            "range": "± 5908",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 89883,
            "range": "± 699",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 89876,
            "range": "± 668",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 89950,
            "range": "± 694",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 5678298,
            "range": "± 51103",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 41,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 763,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 22507,
            "range": "± 214",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 733198,
            "range": "± 7197",
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
            "value": 71,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 215,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 318,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 475,
            "range": "± 7",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2055,
            "range": "± 8",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2235,
            "range": "± 5",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 43477,
            "range": "± 331",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 48739,
            "range": "± 276",
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
            "value": 61,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 138,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2190,
            "range": "± 26",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 51407,
            "range": "± 442",
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
          "id": "4f327b03321faf509d841e50755e340a0064a944",
          "message": "Merge pull request #106 from msmygit/dependabot/github_actions/taiki-e/install-action-2.87.20\n\nchore(deps): bump taiki-e/install-action from 2.85.13 to 2.87.20",
          "timestamp": "2026-09-28T01:21:55Z",
          "url": "https://github.com/msmygit/cdm-rs/commit/4f327b03321faf509d841e50755e340a0064a944"
        },
        "date": 1793008968030,
        "tool": "cargo",
        "benches": [
          {
            "name": "tst_060_passthrough/16",
            "value": 24,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/256",
            "value": 24,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_passthrough/4096",
            "value": 24,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_int_to_text",
            "value": 48,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/1",
            "value": 157,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/16",
            "value": 1616,
            "range": "± 15",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_codec_collection/256",
            "value": 28978,
            "range": "± 659",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/64",
            "value": 197,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/1024",
            "value": 2603,
            "range": "± 9",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_murmur3_ring/65536",
            "value": 163667,
            "range": "± 336",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/64",
            "value": 212,
            "range": "± 11",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/1024",
            "value": 2602,
            "range": "± 144",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_random_ring/65536",
            "value": 163701,
            "range": "± 406",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/64",
            "value": 197,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/4096",
            "value": 10194,
            "range": "± 54",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_narrow_range/65536",
            "value": 10193,
            "range": "± 316",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/1",
            "value": 39,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_extraction/3",
            "value": 40,
            "range": "± 3",
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
            "value": 13,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/8",
            "value": 150,
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
            "value": 544,
            "range": "± 13",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/clean/128",
            "value": 14,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/key_substitution/substituted/128",
            "value": 2233,
            "range": "± 26",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/8",
            "value": 92,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/32",
            "value": 296,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_row/128",
            "value": 1190,
            "range": "± 3",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/value",
            "value": 297,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/null",
            "value": 264,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/bind_unset_decision/empty_collection",
            "value": 314,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/8",
            "value": 7136,
            "range": "± 15",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/8",
            "value": 4067,
            "range": "± 31",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/32",
            "value": 25794,
            "range": "± 118",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/32",
            "value": 15765,
            "range": "± 113",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/cql/128",
            "value": 103388,
            "range": "± 1851",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060/statement_construction/binder/128",
            "value": 64876,
            "range": "± 558",
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
            "value": 196,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_all_columns_match/64",
            "value": 757,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/4",
            "value": 105,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/16",
            "value": 250,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_compare_mismatch_in_last_column/64",
            "value": 821,
            "range": "± 3",
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
            "value": 40836,
            "range": "± 149",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_ratelimit_wait_observer_overhead/instruments",
            "value": 49085,
            "range": "± 874",
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
            "value": 42,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/64",
            "value": 926,
            "range": "± 4",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/1024",
            "value": 12108,
            "range": "± 91",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_murmur3/65536",
            "value": 774867,
            "range": "± 30623",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/1",
            "value": 95262,
            "range": "± 194",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/25",
            "value": 95200,
            "range": "± 191",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_coverage/100",
            "value": 95252,
            "range": "± 445",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/dense",
            "value": 5837716,
            "range": "± 15958",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_split_ring_partition_floor/fallback",
            "value": 36,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/64",
            "value": 1378,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/1024",
            "value": 14814,
            "range": "± 16",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_shuffle_for_run/65536",
            "value": 862659,
            "range": "± 3462",
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
            "value": 58,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_constant_columns/resolve",
            "value": 192,
            "range": "± 6",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/0",
            "value": 281,
            "range": "± 1",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/0",
            "value": 420,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/16",
            "value": 2077,
            "range": "± 51",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/16",
            "value": 2311,
            "range": "± 27",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/field/256",
            "value": 45718,
            "range": "± 250",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_extract_json/pointer/256",
            "value": 45431,
            "range": "± 224",
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
            "value": 25,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_filter_chain/accept_chain_of_four",
            "value": 61,
            "range": "± 0",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/1",
            "value": 130,
            "range": "± 2",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/16",
            "value": 2598,
            "range": "± 77",
            "unit": "ns/iter"
          },
          {
            "name": "tst_060_explode_map/256",
            "value": 46269,
            "range": "± 178",
            "unit": "ns/iter"
          }
        ]
      }
    ]
  }
}