 #!/bin/bash

docker run --name pledge-db \
    -e POSTGRES_PASSWORD=password \
    -e POSTGRES_DB=pledge \
    -p 5432:5432 \
    -v pledge-data:/var/lib/postgresql/data \
    -d postgres:16

# Wait for PostgreSQL to be ready
echo "Waiting for PostgreSQL to start..."
until docker exec pledge-db pg_isready -U postgres > /dev/null 2>&1; do
    sleep 1
done

echo "PostgreSQL is ready!"

# Run the seed script
docker exec -i pledge-db psql -U postgres -d pledge < seed.sql

echo "Database seeded!"
