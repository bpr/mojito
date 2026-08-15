# expect: a floating-point range requires an explicit step
# Upstream's 1/2-argument scalar ranges require an integral dtype.
def main():
    for x in range(Float32(1.5)):
        print(x)
