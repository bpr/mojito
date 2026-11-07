# expect: define 'NOPE' does not exist, please provide it via -D
# A define read with no default fails its instance when the name is not
# defined, as at the pin run without `-D`.
from std.sys import get_defined_string


def main():
    print(get_defined_string["NOPE"]())
