# Example Postgres workload

This is an example workload for testing Postgres on Antithesis. When testing a system
on Antithesis, we set up the system and additionally deploy a client that interacts
with the system under test (SUT) as a normal downstream consumer would (e.g. for Postgres,
an application sitting on top of the database).

Although this specific example is written for Postgres, the workloads here can be repurposed
to a variety of relational databases.

The client executes a series of actions, and then asserts that the behaviour it sees is
legal.

In this Postgres example, the client does two things
- It checks for durability through a simple test where
  - we first create a table `values (id, data, cksum)`
  - throughout the course of the test, we insert and delete rows from the table in the Postgres
    database. The data for these is randomly generated.
  - we maintain a local record of which checksums are present in the database
  - throughout the test, we check that all of the rows the database acknowledged were inserted
    are still present in the database
- We check for consistency through a simple test where
  - we first create a tablet `accounts (id, data, padding)`
  - we create accounts with an initial balance summing to 1,000,000 (e.g.
    `[(id=1, 500,000), (id=2, 500,000)]`)
  - throughout the test, we transfer money between accounts.
  - We then assert that the total balance is constant, and that no account has a negative
    balance (we make sure not to transfer more money than is available in a particular account).
  - The `padding` column is used to force churn and exercise different on-disk storage layouts.
    We fill it with random data.

The test uses Antithesis' [test templates](https://antithesis.com/docs/product/writing_tests/test_templates/).
