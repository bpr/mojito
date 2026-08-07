# expect: reference-valued fields require an explicit origin
# `List[ref T]` storage always names its origin explicitly; a bare reference
# element type in a parameter annotation is rejected.
def stash(mut sink: List[ref Int]):
    pass

def main():
    print(1)
