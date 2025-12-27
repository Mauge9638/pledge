  CREATE TABLE users (
      id SERIAL PRIMARY KEY,
      name TEXT NOT NULL,
      email TEXT
  );

  INSERT INTO users (name, email)
  SELECT
      'User ' || generate_series,
      'user' || generate_series || '@example.com'
  FROM generate_series(1, 100000);
