# PROBE: the pin's decorator warnings.
#
# Both compilers print `1`. The pin also warns "use new_f" at the call of
# the deprecated function. Mojito prints no warning (R1164).
@deprecated("use new_f")
def old_f() -> Int:
    return 1

def main():
    print(old_f())
