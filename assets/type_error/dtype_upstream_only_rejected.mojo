# expect: DType.uint128 is not supported yet
# Upstream defines dtypes Mojito has no representation for; naming one
# rejects explicitly rather than as an unknown member.
def main():
    print(DType.uint128)
