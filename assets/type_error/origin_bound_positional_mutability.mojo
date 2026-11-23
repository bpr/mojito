# Upstream's `Origin` takes `mut` infer-only, before `//`, so a positional
# argument binds no parameter.
# expect: unexpected parameter
def f[o: Origin[True]]():
    pass

def main():
    f()
