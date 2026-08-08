# A t-string captures value snapshots at creation and formats at write time.
# Deviation from real Mojo, recorded in conformance/parity.tsv: Mojo's TString
# holds immutable borrows and its exclusivity rules reject mutating a captured
# value before use; Mojito prints the captured snapshot instead.
def main():
    var x = 1
    var t = t"x={x}"
    x = 2
    print(t)
    print(x)
