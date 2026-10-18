# A t-string captures value snapshots at creation and formats at write time.
# Deviation from real Mojo, recorded in conformance/parity.tsv and
# conformance/assets-mojo-output-diffs.tsv (roadmap R1130): Mojo's TString
# holds references into the origins it is parameterized on, so the pin
# prints `x=2` here; Mojito prints the captured snapshot, `x=1`.
def main():
    var x = 1
    var t = t"x={x}"
    x = 2
    print(t)
    print(x)
