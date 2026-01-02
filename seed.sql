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

  CREATE TABLE posts (
       id SERIAL PRIMARY KEY,
       user_id INTEGER REFERENCES users(id),
       title TEXT NOT NULL,
       content TEXT NOT NULL,
       created_at TIMESTAMP DEFAULT NOW()
   );

   -- Create 500k posts (5 posts per user on average)
   INSERT INTO posts (user_id, title, content, created_at)
   SELECT
       FLOOR(1 + random() * 100000)::int,  -- Fixed: ensures range [1, 100000]
       'Post title ' || generate_series,
       'Lorem ipsum dolor sit amet, consectetur adipiscing elit. ' ||
       'Post content number ' || generate_series || '. ' ||
       REPEAT('Sample text for search testing. ', 5),
       NOW() - (random() * INTERVAL '365 days')
   FROM generate_series(1, 500000);

   -- Add index for faster joins
   CREATE INDEX idx_posts_user_id ON posts(user_id);
