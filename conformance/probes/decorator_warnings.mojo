# PROBE: the pin's decorator warnings.
#
# Both compilers print `1 2`. The pin also warns "'@parameter' is
# deprecated; use '@__parameter'" at the decorator and "use new_f" at the
# call of the deprecated function. Mojito prints no warning (R1164).
@deprecated("use new_f")
def old_f() -> Int:
    return 1

def main():
    @parameter
    def inner() -> Int:
        return 2
    print(old_f(), inner())
