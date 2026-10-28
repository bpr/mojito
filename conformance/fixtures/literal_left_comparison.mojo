# Mojito converts a literal left operand of a sized-scalar comparison to the
# scalar's type. Upstream's `SIMD` declares no reflected comparison, so the
# pin rejects `3 < UInt(4)` with "no matching method in call to '__lt__'".
def main():
    print(3 < UInt(4), 3 == UInt(3))
