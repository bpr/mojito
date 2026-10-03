# Roadmap IDs: the 2026-10-03 conversion

On 2026-10-03 `docs/roadmap.md` replaced positional entry numbers
(`<section>.<n>`, renumbered whenever an entry landed) with stable IDs
(`R<n>`) and tracks, and computed its work order with `scripts/roadmap.py`.
This table maps the numbering of that day to the IDs, for reading older
material: `CHANGELOG.md`, plans, and commit messages cite the old numbers.

A number cited before 2026-10-03 may already have named a different entry
then, because every landing renumbered its section. Confirm by content.

The section-3 entries were spread over the `ownership`, `calls`,
`comptime`, and `mojo-shape` tracks. Four multi-problem ledgers became one
entry per problem: the `mojo-shape` track (3.76), the `divergences` track
(3.77), and the start of the `stdlib` track (4.1, 4.2).

| Old | ID |
| --- | --- |
| 1.1 | R1 |
| 1.2 | R2 |
| 1.3 | R3 |
| 1.4 | R4 |
| 1.5 | R5 |
| 1.6 | R6 |
| 1.7 | R7 |
| 1.8 | R8 |
| 1.9 | R9 |
| 1.10 | R10 |
| 1.11 | R11 |
| 1.12 | R12 |
| 1.13 | R13 |
| 1.14 | R14 |
| 1.15 | R15 |
| 1.16 | R16 |
| 1.17 | R17 |
| 2.1 | R18 |
| 2.2 | R19 |
| 3.1 | R20 |
| 3.2 | R21 |
| 3.3 | R22 |
| 3.4 | R23 |
| 3.5 | R24 |
| 3.6 | R25 |
| 3.7 | R26 |
| 3.8 | R27 |
| 3.9 | R28 |
| 3.10 | R29 |
| 3.11 | R30 |
| 3.12 | R31 |
| 3.13 | R32 |
| 3.14 | R33 |
| 3.15 | R34 |
| 3.16 | R35 |
| 3.17 | R36 |
| 3.18 | R37 |
| 3.19 | R38 |
| 3.20 | R39 |
| 3.21 | R40 |
| 3.22 | R41 |
| 3.23 | R42 |
| 3.24 | R43 |
| 3.25 | R44 |
| 3.26 | R45 |
| 3.27 | R46 |
| 3.28 | R47 |
| 3.29 | R48 |
| 3.30 | R49 |
| 3.31 | R50 |
| 3.32 | R51 |
| 3.33 | R52 |
| 3.34 | R53 |
| 3.35 | R54 |
| 3.36 | R55 |
| 3.37 | R56 |
| 3.38 | R57 |
| 3.39 | R58 |
| 3.40 | R59 |
| 3.41 | R60 |
| 3.42 | R61 |
| 3.43 | R62 |
| 3.44 | R63 |
| 3.45 | R64 |
| 3.46 | R65 |
| 3.47 | R66 |
| 3.48 | R67 |
| 3.49 | R68 |
| 3.50 | R69 |
| 3.51 | R70 |
| 3.52 | R71 |
| 3.53 | R72 |
| 3.54 | R73 |
| 3.55 | R74 |
| 3.56 | R75 |
| 3.57 | R76 |
| 3.58 | R77 |
| 3.59 | R78 |
| 3.60 | R79 |
| 3.61 | R80 |
| 3.62 | R81 |
| 3.63 | R82 |
| 3.64 | R83 |
| 3.65 | R84 |
| 3.66 | R85 |
| 3.67 | R86 |
| 3.68 | R87 |
| 3.69 | R88 |
| 3.70 | R89 |
| 3.71 | R90 |
| 3.72 | R91 |
| 3.73 | R92 |
| 3.74 | R93 |
| 3.75 | R94 |
| 3.76 | split into R159–R161 |
| 3.77 | split into R162–R195 |
| 3.78 | R97 |
| 3.79 | R98 |
| 3.80 | R99 |
| 3.81 | R100 |
| 3.82 | R101 |
| 3.83 | R102 |
| 3.84 | R103 |
| 3.85 | R104 |
| 3.86 | R105 |
| 3.87 | R106 |
| 3.88 | R107 |
| 3.89 | R108 |
| 3.90 | R109 |
| 3.91 | R110 |
| 3.92 | R111 |
| 3.93 | R112 |
| 3.94 | R113 |
| 3.95 | R114 |
| 3.96 | R115 |
| 3.97 | R116 |
| 3.98 | R117 |
| 3.99 | R118 |
| 3.100 | R119 |
| 3.101 | R120 |
| 3.102 | R121 |
| 3.103 | R122 |
| 3.104 | R123 |
| 3.105 | R124 |
| 3.106 | R125 |
| 3.107 | R126 |
| 3.108 | R127 |
| 3.109 | R128 |
| 3.110 | R129 |
| 3.111 | R130 |
| 3.112 | R131 |
| 3.113 | R132 |
| 3.115 | R133 |
| 3.116 | R134 |
| 3.117 | R135 |
| 3.118 | R136 |
| 3.119 | R137 |
| 3.120 | R138 |
| 3.121 | R139 |
| 3.122 | R140 |
| 4.1 | split into R196–R225 (R215 later folded into R97) |
| 4.2 | split into R226–R244 |
| 4.3 | R143 |
| 4.4 | R144 |
| 5.1 | R145 |
| 5.2 | R146 |
| 5.3 | R147 |
| 5.4 | R148 |
| 5.5 | R149 |
| 5.6 | R150 |
| 5.7 | R151 |
| 5.8 | R152 |
| 5.9 | R153 |
| 5.10 | R154 |
| 5.11 | R155 |
| 5.12 | R156 |
| 5.13 | R157 |
| 6.1 | R158 |
