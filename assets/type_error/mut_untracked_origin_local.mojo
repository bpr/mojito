# expect: doesn't match expected origin 'MutUntrackedOrigin'
# A tracked local place does not convert to a `ref[MutUntrackedOrigin]` parameter.
def mutate_untracked(ref[MutUntrackedOrigin] value: Int):
    value += 1
def main():
    var value = 41
    mutate_untracked(value)
    print(value)
