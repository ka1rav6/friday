# Notes: io_uring vs epoll

Taken while reading the kernel docs. Unverified recollection, not a citation.

- epoll is readiness-based: the kernel says a fd is ready, you then syscall to
  do the work. Two trips per operation.
- io_uring is completion-based: you submit the operation itself into a ring and
  collect the result later. Batches amortise the syscall.
- The win shows up when the per-operation syscall cost dominates — many small
  reads, high connection counts.
- The cost is portability and a much larger API surface. epoll is everywhere;
  io_uring needs a recent kernel and has had its share of CVEs.

Open question: at what request rate does the difference actually matter for our
workload? Needs measurement, not opinion.
