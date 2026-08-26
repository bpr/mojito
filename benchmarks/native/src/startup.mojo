# Benchmark: startup-dominated. main does near-zero work and prints one
# small computed value.
def main():
    var x: Int = 1
    var i: Int = 0
    while i < 100:
        x = (x * 31 + 7) % 1009
        i += 1
    print("startup:", x)
