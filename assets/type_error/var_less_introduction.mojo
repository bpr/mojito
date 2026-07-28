# expect: var count
# A var-less introduction (`count = 10` on an undeclared name) is rejected:
# Mojito requires `var` to declare a new variable, matching Mojo's move to a
# single declaration pathway.
def main():
    count = 10
    print(count)
